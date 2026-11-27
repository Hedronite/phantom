//! `impact --diff`: what the work tree's change against a base reaches
//! (SPEC-w9 G2b).
//!
//! The changed symbols come from `phantom_fractal_extract::diff`, which
//! compares raw bytes with the base commit's tree and extracts both sides
//! in memory, reading no git index and running no git filter. The walk is
//! `phantom_fractal_analyze::impact_many`, from every changed symbol the
//! index holds at once. Nothing is written, and no path is absolute.

use std::cmp::Reverse;
use std::path::PathBuf;

use phantom_fractal_analyze::{DiffState, ImpactOptions, impact_many};
use phantom_fractal_extract::{DiffError, Registry, default_registry, diff_with};
use phantom_fractal_git::{Base, ChangesError};
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::impact::{
    CountsOut, GroupOut, OptionsOut, ReasonOut, Shown, TestOut, counts_out, group_out, options_out,
    reason_out, test_out,
};
use super::lookup::current_index;
use super::{Failure, Reply, canonical_repo, fresh};

/// The one message for `--staged` and `--unstaged`.
const STAGED: &str = "staged and unstaged diffs need the git index, which this build does not read";

/// The one message for a tree that is not the top of a git work tree.
const NOT_GIT: &str =
    "impact --diff needs a git root: the indexed tree is not the top of a git work tree";

/// The one message for a history that could not be read; the error's own
/// text may hold a path.
const UNREADABLE: &str = "the git history could not be read for the diff";

/// What `impact --diff` is asked for.
pub(crate) struct DiffRequest {
    /// The base: a branch, a tag, or a commit id of at least 7 hex digits.
    /// None is HEAD.
    pub(crate) against: Option<String>,
    /// The indexed tree, which must be a git root.
    pub(crate) repo: PathBuf,
    /// The walk's depth, confidence floor, and whether tests count.
    pub(crate) options: ImpactOptions,
    /// Drop `groups` and `tests` from `impact`.
    pub(crate) summary: bool,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of `impact --diff`, with its keys in this order.
#[derive(Clone, Serialize)]
pub(crate) struct DiffResult {
    base: BaseOut,
    changed: Vec<ChangedOut>,
    files_only: Vec<FileOut>,
    manifest_changed: bool,
    impact: ImpactOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

/// The commit the work tree was compared with.
#[derive(Clone, Serialize)]
struct BaseOut {
    /// `head`, or `ref` for `--against`.
    kind: &'static str,
    /// The name `--against` gave, or null for HEAD.
    #[serde(rename = "ref")]
    name: Option<String>,
    oid: String,
}

/// One changed symbol, and whether the index holds its key.
#[derive(Clone, Serialize)]
struct ChangedOut {
    path: String,
    key: String,
    name: String,
    change: &'static str,
    in_index: bool,
}

/// A changed file with no changed symbol: not `.rs`, or a `.rs` change
/// that touches no symbol's bytes.
#[derive(Clone, Serialize)]
struct FileOut {
    path: String,
    change: &'static str,
}

/// The walk from every changed symbol in the index, in `impact`'s shape
/// without its one `symbol`. `groups` and `tests` are absent under
/// `--summary`, and `tests` under `--no-tests`.
#[derive(Clone, Serialize)]
struct ImpactOut {
    risk: &'static str,
    exact: bool,
    reasons: Vec<ReasonOut>,
    counts: CountsOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    groups: Option<Vec<GroupOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tests: Option<Vec<TestOut>>,
    options: OptionsOut,
}

/// Under a budget, the deepest group's hits go first, from the end, then
/// the next group's, then the tests, then `files_only`, then `changed`,
/// each from the end. The base, the counts, the risk and the reasons stay.
impl Shed for DiffResult {
    fn sheddable(&self) -> usize {
        let hits: usize = self
            .impact
            .groups
            .iter()
            .flatten()
            .map(|group| group.hits.len())
            .sum();
        hits + self.impact.tests.as_ref().map_or(0, Vec::len)
            + self.files_only.len()
            + self.changed.len()
    }

    fn shed(&mut self, mut count: usize) {
        let mut groups: Vec<&mut GroupOut> = self.impact.groups.iter_mut().flatten().collect();
        groups.sort_by_key(|group| Reverse(group.depth));
        for group in groups {
            drop_last(&mut group.hits, &mut count);
        }
        if let Some(tests) = &mut self.impact.tests {
            drop_last(tests, &mut count);
        }
        drop_last(&mut self.files_only, &mut count);
        drop_last(&mut self.changed, &mut count);
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// The usage failure for `--staged` and `--unstaged`.
pub(crate) fn staged_refusal() -> Failure {
    Failure::new(ErrorKind::Usage, STAGED)
}

/// What the change in the work tree of `request.repo` against its base
/// reaches, fitted to `request.max_tokens`.
pub(crate) fn impact_diff(request: &DiffRequest) -> Result<Reply<DiffResult>, Failure> {
    let registry = default_registry()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    impact_diff_with(request, &registry)
}

pub(crate) fn impact_diff_with(
    request: &DiffRequest,
    registry: &Registry<'_>,
) -> Result<Reply<DiffResult>, Failure> {
    let (store, names, root) = open_diff_context(request)?;
    let (result, meta) = compute_diff_impact(&store, &names, &root, request, registry)?;
    let result = budget::fit(result, request.max_tokens, &meta)?;
    Ok(Reply::with_meta(result, meta))
}

fn open_diff_context(
    request: &DiffRequest,
) -> Result<(phantom_fractal_store::Store, super::lookup::Names, PathBuf), Failure> {
    check_diff_request(request)?;
    let (store, names) = current_index(&request.repo)?;
    let root = canonical_repo(&request.repo)?;
    Ok((store, names, root))
}

fn compute_diff_impact(
    store: &phantom_fractal_store::Store,
    names: &super::lookup::Names,
    root: &std::path::Path,
    request: &DiffRequest,
    registry: &Registry<'_>,
) -> Result<(DiffResult, phantom_fractal_proto::Meta), Failure> {
    let changes = compute_diff(root, request.against.as_deref(), registry)?;
    let (changed, sources) = map_changed_symbols(store, &changes.changed)?;
    let meta = fresh::meta(root, store)?;
    let state = build_diff_state(
        &changed,
        &changes.files_only,
        changes.manifest_changed,
        meta.stale,
        registry,
    );
    let answer = impact_many(store, &sources, &request.options, &state)?;
    Ok((
        assemble_diff_result(request, changes, changed, answer, names),
        meta,
    ))
}

fn check_diff_request(request: &DiffRequest) -> Result<(), Failure> {
    if !request.options.in_range() {
        return Err(Failure::new(
            ErrorKind::Usage,
            "depth must be 1 to 5 and min_confidence above 0 and at most 1",
        ));
    }
    budget::check(request.max_tokens)
}

fn compute_diff(
    root: &std::path::Path,
    against: Option<&str>,
    registry: &Registry<'_>,
) -> Result<phantom_fractal_extract::Diff, Failure> {
    let base = match against {
        None => Base::Head,
        Some(name) => Base::Ref(name.to_owned()),
    };
    diff_with(root, &base, registry)
        .map_err(diff_failure)?
        .ok_or_else(|| Failure::new(ErrorKind::Usage, NOT_GIT))
}

fn map_changed_symbols(
    store: &phantom_fractal_store::Store,
    symbols: &[phantom_fractal_extract::ChangedSymbol],
) -> Result<(Vec<ChangedOut>, Vec<i64>), Failure> {
    let mut changed = Vec::new();
    let mut sources = Vec::new();
    for symbol in symbols {
        let id = store.symbol_by_stable_key(&symbol.key)?;
        sources.extend(id);
        changed.push(ChangedOut {
            path: symbol.path.clone(),
            key: symbol.key.clone(),
            name: symbol.name.clone(),
            change: symbol.change.as_str(),
            in_index: id.is_some(),
        });
    }
    Ok((changed, sources))
}

fn assemble_diff_result(
    request: &DiffRequest,
    changes: phantom_fractal_extract::Diff,
    changed: Vec<ChangedOut>,
    answer: phantom_fractal_analyze::ManyImpact,
    names: &super::lookup::Names,
) -> DiffResult {
    let (groups, tests) = format_diff_hits(request, &answer, names);
    DiffResult {
        base: BaseOut {
            kind: if request.against.is_some() {
                "ref"
            } else {
                "head"
            },
            name: request.against.clone(),
            oid: changes.base_oid,
        },
        changed,
        files_only: changes
            .files_only
            .into_iter()
            .map(|file| FileOut {
                path: file.path,
                change: file.change.as_str(),
            })
            .collect(),
        manifest_changed: changes.manifest_changed,
        impact: ImpactOut {
            risk: answer.risk.as_str(),
            exact: answer.exact,
            reasons: answer.reasons.into_iter().map(reason_out).collect(),
            counts: counts_out(answer.counts),
            groups,
            tests,
            options: options_out(&request.options),
        },
        tokens: None,
    }
}

fn format_diff_hits(
    request: &DiffRequest,
    answer: &phantom_fractal_analyze::ManyImpact,
    names: &super::lookup::Names,
) -> (Option<Vec<GroupOut>>, Option<Vec<TestOut>>) {
    if request.summary {
        return (None, None);
    }
    let groups = answer
        .groups
        .iter()
        .cloned()
        .map(|group| group_out(names, group, Shown::Plain))
        .collect();
    let tests = request.options.include_tests.then(|| {
        answer
            .tests
            .iter()
            .cloned()
            .map(|hit| test_out(names, hit, Shown::Plain))
            .collect()
    });
    (Some(groups), tests)
}

/// A base that names no commit, or more than one, is usage with its own
/// path-free text; any other failure is usage with a fixed text.
fn build_diff_state(
    changed: &[ChangedOut],
    files_only: &[phantom_fractal_extract::FileOnly],
    manifest_changed: bool,
    stale_index: bool,
    registry: &Registry<'_>,
) -> DiffState {
    let unindexed = changed.iter().filter(|symbol| !symbol.in_index).count();
    let supported_only = files_only
        .iter()
        .filter(|file| registry.is_supported(&file.path))
        .count();
    DiffState {
        stale_index,
        unindexed_changes: u64::try_from(unindexed + supported_only).unwrap_or(u64::MAX),
        manifest_changed,
    }
}

fn diff_failure(err: DiffError) -> Failure {
    match err {
        DiffError::Changes(err @ (ChangesError::UnknownBase | ChangesError::AmbiguousBase)) => {
            Failure::new(ErrorKind::Usage, err.to_string())
        }
        _ => Failure::new(ErrorKind::Usage, UNREADABLE),
    }
}

#[cfg(test)]
mod tests {
    use phantom_fractal_extract::{FileOnly, Registry, RustDriver};
    use phantom_fractal_git::FileChange;

    use super::{ChangedOut, build_diff_state};

    #[test]
    fn build_diff_state_never_drops_supported_files_only_or_manifest_with_empty_sources() {
        let rust = RustDriver::owned().expect("rust driver");
        let registry = Registry::new(vec![&rust]);

        // Only a supported .rs file changed outside symbol spans, with clean index: must force unindexed_changes.
        let state = build_diff_state(
            &[],
            &[
                FileOnly {
                    path: "README.md".to_owned(),
                    change: FileChange::Modified,
                },
                FileOnly {
                    path: "src/lib.rs".to_owned(),
                    change: FileChange::Modified,
                },
            ],
            false,
            false,
            &registry,
        );
        assert_eq!(state.unindexed_changes, 1);
        assert!(!state.manifest_changed);

        // Manifest-only change with no indexed symbol sources: sets manifest_changed = true.
        let man = build_diff_state(
            &[],
            &[FileOnly {
                path: "Cargo.toml".to_owned(),
                change: FileChange::Modified,
            }],
            true,
            false,
            &registry,
        );
        assert!(man.manifest_changed);
        assert_eq!(man.unindexed_changes, 0);

        // Non-empty changed symbols MUST still propagate supported files_only and manifest_changed.
        let with_sources = build_diff_state(
            &[ChangedOut {
                path: "src/a.rs".to_owned(),
                key: "k1".to_owned(),
                name: "f".to_owned(),
                change: "modified",
                in_index: true,
            }],
            &[FileOnly {
                path: "src/c.rs".to_owned(),
                change: FileChange::Modified,
            }],
            true,
            true,
            &registry,
        );
        assert!(with_sources.stale_index);
        assert!(with_sources.manifest_changed);
        assert_eq!(with_sources.unindexed_changes, 1);

        let bad_depth = super::DiffRequest {
            against: None,
            repo: std::path::PathBuf::from("/nonexistent"),
            options: phantom_fractal_analyze::ImpactOptions {
                max_depth: 0,
                ..phantom_fractal_analyze::ImpactOptions::default()
            },
            summary: false,
            max_tokens: None,
        };
        let Err(err) = super::impact_diff_with(&bad_depth, &registry) else {
            panic!("invalid depth must fail");
        };
        assert_eq!(err.kind, phantom_fractal_proto::ErrorKind::Usage);

        let bad_budget = super::DiffRequest {
            against: None,
            repo: std::path::PathBuf::from("/nonexistent"),
            options: phantom_fractal_analyze::ImpactOptions::default(),
            summary: false,
            max_tokens: Some(0),
        };
        let Err(err_b) = super::impact_diff_with(&bad_budget, &registry) else {
            panic!("invalid budget must fail");
        };
        assert_eq!(err_b.kind, phantom_fractal_proto::ErrorKind::Usage);
    }
}
