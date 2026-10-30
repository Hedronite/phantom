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
use phantom_fractal_extract::{DiffError, diff};
use phantom_fractal_git::{Base, ChangesError};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::impact::{
    CountsOut, GroupOut, OptionsOut, ReasonOut, TestOut, counts_out, group_out, options_out,
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
    if !request.options.in_range() {
        return Err(Failure::new(
            ErrorKind::Usage,
            "depth must be 1 to 5 and min_confidence above 0 and at most 1",
        ));
    }
    budget::check(request.max_tokens)?;
    let (store, names) = current_index(&request.repo)?;
    let root = canonical_repo(&request.repo)?;
    let language = RustLanguage::new()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    let base = match &request.against {
        None => Base::Head,
        Some(name) => Base::Ref(name.clone()),
    };
    let changes = diff(&root, &base, &language)
        .map_err(diff_failure)?
        .ok_or_else(|| Failure::new(ErrorKind::Usage, NOT_GIT))?;

    let mut changed = Vec::new();
    let mut sources = Vec::new();
    for symbol in changes.changed {
        let id = store.symbol_by_stable_key(&symbol.key)?;
        sources.extend(id);
        changed.push(ChangedOut {
            path: symbol.path,
            key: symbol.key,
            name: symbol.name,
            change: symbol.change.as_str(),
            in_index: id.is_some(),
        });
    }
    let unindexed = changed.iter().filter(|symbol| !symbol.in_index).count();
    let meta = fresh::meta(&root, &store)?;
    let state = DiffState {
        stale_index: meta.stale,
        unindexed_changes: u64::try_from(unindexed).unwrap_or(u64::MAX),
    };
    let options = request.options;
    let answer = impact_many(&store, &sources, &options, &state)?;
    let (groups, tests) = if request.summary {
        (None, None)
    } else {
        let groups = answer
            .groups
            .into_iter()
            .map(|group| group_out(&names, group))
            .collect();
        let tests = options.include_tests.then(|| {
            answer
                .tests
                .into_iter()
                .map(|hit| test_out(&names, hit))
                .collect()
        });
        (Some(groups), tests)
    };
    let result = DiffResult {
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
            options: options_out(&options),
        },
        tokens: None,
    };
    let result = budget::fit(result, request.max_tokens, &meta)?;
    Ok(Reply::with_meta(result, meta))
}

/// A base that names no commit, or more than one, is usage with its own
/// path-free text; any other failure is usage with a fixed text.
fn diff_failure(err: DiffError) -> Failure {
    match err {
        DiffError::Changes(err @ (ChangesError::UnknownBase | ChangesError::AmbiguousBase)) => {
            Failure::new(ErrorKind::Usage, err.to_string())
        }
        _ => Failure::new(ErrorKind::Usage, UNREADABLE),
    }
}
