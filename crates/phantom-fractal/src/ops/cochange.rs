//! `cochange`: the files that tend to change with one file, from the
//! co-change scan `index` stored on a git root (SPEC-w8 G3a).
//!
//! Nothing here reads git or writes the index. Without a scan the answer
//! is empty, with `scan` null.

use std::path::PathBuf;

use phantom_fractal_git::Window;
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
/// changed together, in unix seconds.
#[derive(Clone, Serialize)]
pub(crate) struct PartnerOut {
    path: String,
    together: f64,
    lift: f64,
    conf: f64,
    last_time: i64,
}

/// The partners of `request.file`, best lift first, then the most changes
/// together, then path, fitted to `request.max_tokens`.
pub(crate) fn cochange(request: &CochangeRequest) -> Result<Reply<CochangeResult>, Failure> {
    if !(LIMIT_MIN..=LIMIT_MAX).contains(&request.limit) {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!("limit must be {LIMIT_MIN} to {LIMIT_MAX}"),
        ));
    }
    if !min_lift_in_range(request.min_lift) {
        return Err(Failure::new(ErrorKind::Usage, min_lift_usage()));
    }
    budget::check(request.max_tokens)?;
    let store = current_store(&request.repo)?;
    let file = indexed_file(&store, &request.file)?;
    let scan = scan(&store)?;
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let mut partners = if scan.is_some() {
        partners(&store, &file, request.min_lift, request.limit + 1)?
    } else {
        Vec::new()
    };
    let truncated = partners.len() > limit;
    partners.truncate(limit);
    let result = CochangeResult {
        file,
        scan,
        partners,
        truncated,
        tokens: None,
    };
    answer(result, &store, &request.repo, request.max_tokens)
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

/// `path` as the store holds it, with any leading `./` removed, or
/// `NotFound`. The message never holds the path, which may be absolute.
fn indexed_file(store: &Store, path: &str) -> Result<String, Failure> {
    let mut relative = path;
    while let Some(rest) = relative.strip_prefix("./") {
        relative = rest;
    }
    store
        .files()?
        .into_iter()
        .map(|file| file.path)
        .find(|stored| stored == relative)
        .ok_or_else(|| {
            Failure::new(
                ErrorKind::NotFound,
                "file is not an indexed file; give a path relative to the repo",
            )
        })
}

/// The stored scan's HEAD and window, or none without a scan.
fn scan(store: &Store) -> Result<Option<ScanOut>, Failure> {
    let Some(scan) = store.cochange_scan()? else {
        return Ok(None);
    };
    // A cleared scan has an empty HEAD.
    let Some(head) = scan.last_oid.filter(|head| !head.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(ScanOut {
        head,
        window: stored_window(&scan.window_json)?.into(),
    }))
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
            })
        })
        .collect())
}
