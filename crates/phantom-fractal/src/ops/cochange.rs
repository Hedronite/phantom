//! `cochange`: the files that tend to change with one file, from the
//! co-change scan `index` stored on a git root (SPEC-w8 G3a).
//!
//! Nothing here reads git or writes the index. Without a scan the answer
//! is empty, with `scan` null. The reply also says what the scores are
//! over, in `scope`, and gives each partner's decayed weight again as
//! `together_decayed` (SPEC-w14 G3b); `together` stays as its alias.
//! `cochange --count` is [`super::count`].

use std::path::PathBuf;

use phantom_fractal_git::{BULK_FILES, MIN_TOGETHER, Window};
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::Store;
use serde::{Deserialize, Serialize};

use super::budget::{self, Shed, Tokens, drop_last};
use super::index::WindowOut;
use super::lookup::current_store;
use super::{Failure, Reply, answer};

/// The fewest and most partners one `cochange` returns.
pub(crate) const LIMIT_MIN: u64 = 1;
pub(crate) const LIMIT_MAX: u64 = 100;
/// The partners `cochange` returns when no limit is given.
pub(crate) const DEFAULT_LIMIT: u64 = 20;
/// The lowest lift `cochange` and `show` list when none is given.
pub(crate) const DEFAULT_MIN_LIFT: f64 = 1.5;
/// The highest `min_lift` the command line and MCP take (SPEC-w10 G0b).
pub(crate) const MIN_LIFT_MAX: u32 = 1000;

/// What `cochange` is asked for.
pub(crate) struct CochangeRequest {
    /// An indexed file, relative to the repo.
    pub(crate) file: String,
    /// The lowest lift listed: a finite number, at least 0.
    pub(crate) min_lift: f64,
    /// [`LIMIT_MIN`] to [`LIMIT_MAX`].
    pub(crate) limit: u64,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `cochange`, with its keys in this order.
#[derive(Clone, Serialize)]
pub(crate) struct CochangeResult {
    /// The file as indexed.
    file: String,
    /// The stored scan, or null when there is none.
    scan: Option<ScanOut>,
    partners: Vec<PartnerOut>,
    /// True when more partners passed `min_lift` than `partners` holds.
    truncated: bool,
    /// What the scores are over.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<Scope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

impl Shed for CochangeResult {
    fn sheddable(&self) -> usize {
        self.partners.len()
    }

    fn shed(&mut self, mut count: usize) {
        drop_last(&mut self.partners, &mut count);
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }

    fn drop_extras(&mut self) {
        self.scope = None;
        for partner in &mut self.partners {
            partner.together_decayed = None;
        }
    }
}

/// What a co-change answer counts: which files, how commits are weighted,
/// the fewest shared commits a partner needs, and the most files a commit
/// may change before it is skipped as bulk.
#[derive(Clone, Serialize)]
pub(crate) struct Scope {
    files: &'static str,
    weighting: &'static str,
    min_commits: u64,
    bulk_skip: usize,
}

impl Scope {
    /// The stored scan's: indexed Rust files, decayed, at least
    /// [`MIN_TOGETHER`] shared commits.
    pub(crate) const fn decayed() -> Self {
        Self {
            files: "indexed .rs",
            weighting: "decay 90d",
            min_commits: MIN_TOGETHER,
            bulk_skip: BULK_FILES,
        }
    }

    /// `--count`'s: every tracked file, one per shared commit, from one.
    pub(crate) const fn count() -> Self {
        Self {
            files: "tracked",
            weighting: "none",
            min_commits: 1,
            bulk_skip: BULK_FILES,
        }
    }
}

/// The scan a co-change answer comes from: its HEAD and window.
#[derive(Clone, Serialize)]
struct ScanOut {
    head: String,
    window: WindowOut,
}

/// The stored `window_json`, of which the days and the commit cap are
/// printed.
#[derive(Deserialize)]
struct StoredWindow {
    days: u32,
    max_commits: u32,
}

/// One partner: the other file, the decayed weight of the commits the two
/// changed in together, the pair's lift, the confidence that a change to
/// the queried file comes with one to this file, and the last time they
/// changed together, in unix seconds. `cochange` also names the weight
/// `together_decayed`; `show` leaves that out.
#[derive(Clone, Serialize)]
pub(crate) struct PartnerOut {
    path: String,
    together: f64,
    lift: f64,
    conf: f64,
    last_time: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    together_decayed: Option<f64>,
}

/// The partners of `request.file`, best lift first, then the most changes
/// together, then path, fitted to `request.max_tokens`.
pub(crate) fn cochange(request: &CochangeRequest) -> Result<Reply<CochangeResult>, Failure> {
    checked(request)?;
    let store = current_store(&request.repo)?;
    let file = indexed_file(&store, &request.file)?;
    let scan = scan(&store)?;
    let (partners, truncated) = if scan.is_some() {
        listed(&store, &file, request)?
    } else {
        (Vec::new(), false)
    };
    let result = CochangeResult {
        file,
        scan,
        partners,
        truncated,
        scope: Some(Scope::decayed()),
        tokens: None,
    };
    answer(result, &store, &request.repo, request.max_tokens)
}

/// `Usage` unless the limit, `min_lift` and `max_tokens` are in range.
fn checked(request: &CochangeRequest) -> Result<(), Failure> {
    check_limit(request.limit)?;
    if !min_lift_in_range(request.min_lift) {
        return Err(Failure::new(ErrorKind::Usage, min_lift_usage()));
    }
    budget::check(request.max_tokens)
}

/// `file`'s first `request.limit` partners of at least `request.min_lift`,
/// each with its weight as `together_decayed` too, and whether there were
/// more.
fn listed(
    store: &Store,
    file: &str,
    request: &CochangeRequest,
) -> Result<(Vec<PartnerOut>, bool), Failure> {
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let mut partners = partners(store, file, request.min_lift, request.limit + 1)?;
    let truncated = partners.len() > limit;
    partners.truncate(limit);
    for partner in &mut partners {
        partner.together_decayed = Some(partner.together);
    }
    Ok((partners, truncated))
}

/// `Usage` unless `limit` is [`LIMIT_MIN`] to [`LIMIT_MAX`].
pub(crate) fn check_limit(limit: u64) -> Result<(), Failure> {
    if (LIMIT_MIN..=LIMIT_MAX).contains(&limit) {
        Ok(())
    } else {
        Err(Failure::new(
            ErrorKind::Usage,
            format!("limit must be {LIMIT_MIN} to {LIMIT_MAX}"),
        ))
    }
}

/// True for a finite `min_lift` from 0 to [`MIN_LIFT_MAX`].
pub(crate) fn min_lift_in_range(min_lift: f64) -> bool {
    min_lift.is_finite() && (0.0..=f64::from(MIN_LIFT_MAX)).contains(&min_lift)
}

/// The usage message for a `min_lift` out of range, the same in the
/// command line and MCP.
pub(crate) fn min_lift_usage() -> String {
    format!("min_lift must be a number from 0 to {MIN_LIFT_MAX}")
}

/// `path` with any leading `./` removed.
pub(crate) fn relative(path: &str) -> &str {
    let mut relative = path;
    while let Some(rest) = relative.strip_prefix("./") {
        relative = rest;
    }
    relative
}

/// `path` as the store holds it, with any leading `./` removed, or
/// `NotFound`, which says co-change is kept only for indexed files and
/// points to `--count` and git. The message never holds the path, which
/// may be absolute.
fn indexed_file(store: &Store, path: &str) -> Result<String, Failure> {
    let relative = relative(path);
    store
        .files()?
        .into_iter()
        .map(|file| file.path)
        .find(|stored| stored == relative)
        .ok_or_else(|| {
            Failure::new(
                ErrorKind::NotFound,
                "file is not indexed at HEAD: co-change is kept only for indexed Rust files; \
                 give a path relative to the repo, or for raw counts over every tracked file \
                 use cochange --count or git log",
            )
        })
}

/// The stored scan's HEAD and window, or none without a scan.
fn scan(store: &Store) -> Result<Option<ScanOut>, Failure> {
    Ok(stored_scan(store)?.map(|(head, window)| ScanOut {
        head,
        window: window.into(),
    }))
}

/// The stored scan's HEAD and the window it read, or none without a scan.
pub(crate) fn stored_scan(store: &Store) -> Result<Option<(String, Window)>, Failure> {
    let Some(scan) = store.cochange_scan()? else {
        return Ok(None);
    };
    // A cleared scan has an empty HEAD.
    let Some(head) = scan.last_oid.filter(|head| !head.is_empty()) else {
        return Ok(None);
    };
    Ok(Some((head, stored_window(&scan.window_json)?)))
}

/// The window a stored scan read, from its `window_json`.
pub(crate) fn stored_window(window_json: &str) -> Result<Window, Failure> {
    let window: StoredWindow = serde_json::from_str(window_json).map_err(|_| {
        Failure::new(
            ErrorKind::Usage,
            "store: the co-change window could not be read",
        )
    })?;
    Ok(Window {
        days: window.days,
        max_commits: window.max_commits,
    })
}

/// The stored partners of `path` with a lift of at least `min_lift`, at
/// most `limit`, in the store's order.
pub(crate) fn partners(
    store: &Store,
    path: &str,
    min_lift: f64,
    limit: u64,
) -> Result<Vec<PartnerOut>, Failure> {
    let limit = u32::try_from(limit).unwrap_or(u32::MAX);
    Ok(store
        .cochange_of(path, min_lift, limit)?
        .into_iter()
        .filter_map(|partner| {
            Some(PartnerOut {
                path: partner.path?,
                together: partner.together,
                lift: partner.lift,
                conf: partner.conf_of,
                last_time: partner.last_time,
                together_decayed: None,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use phantom_fractal_git::{BULK_FILES, HALF_LIFE_DAYS, MIN_TOGETHER};

    use std::path::PathBuf;

    use super::{CochangeRequest, Scope, check_limit, checked};

    #[test]
    fn a_limit_is_1_to_100() {
        for limit in [0, 101, u64::MAX] {
            assert!(check_limit(limit).is_err(), "{limit}");
        }
        for limit in [1, 20, 100] {
            assert!(check_limit(limit).is_ok(), "{limit}");
        }
    }

    #[test]
    fn a_request_out_of_range_is_refused_before_any_read() {
        let request = |limit: u64, min_lift: f64, max_tokens: Option<u64>| CochangeRequest {
            file: "src/lib.rs".to_owned(),
            min_lift,
            limit,
            repo: PathBuf::from("."),
            max_tokens,
        };
        assert!(checked(&request(20, 1.5, None)).is_ok());
        assert!(checked(&request(100, 0.0, Some(100_000))).is_ok());
        for bad in [
            request(0, 1.5, None),
            request(20, -1.0, None),
            request(20, f64::NAN, None),
            request(20, 1.5, Some(0)),
        ] {
            assert!(checked(&bad).is_err());
        }
    }

    #[test]
    fn the_scopes_name_the_git_crate_constants() {
        assert!(
            (HALF_LIFE_DAYS - 90.0).abs() < f64::EPSILON,
            "the label says 90d"
        );
        assert_eq!(
            serde_json::to_value(Scope::decayed()).expect("json"),
            serde_json::json!({
                "files": "indexed .rs",
                "weighting": "decay 90d",
                "min_commits": MIN_TOGETHER,
                "bulk_skip": BULK_FILES,
            })
        );
        assert_eq!(
            serde_json::to_value(Scope::count()).expect("json"),
            serde_json::json!({
                "files": "tracked",
                "weighting": "none",
                "min_commits": 1,
                "bulk_skip": BULK_FILES,
            })
        );
    }
}
