//! `cochange --count` (SPEC-w14 G3b): for one file, how many kept commits
//! each other tracked file shared with it, with no decay and no lift
//! floor, over the window the stored scan read.
//!
//! The counts are computed at query time by `phantom-fractal-git`, which
//! reads the history in process and writes nothing. Without a stored
//! scan, or on a tree that is no longer a git root, `scan` is null and
//! there are no partners.

use std::path::PathBuf;

use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::Store;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::cochange::{Scope, check_limit, relative, stored_scan};
use super::index::WindowOut;
use super::lookup::current_store;
use super::{Failure, Reply, answer, canonical_repo};

/// What `cochange --count` is asked for.
pub(crate) struct CountRequest {
    /// A tracked file, relative to the repo; it need not be indexed.
    pub(crate) file: String,
    /// 1 to 100, as `cochange` takes it.
    pub(crate) limit: u64,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `cochange --count`, with its keys in this order.
#[derive(Clone, Serialize)]
pub(crate) struct CountResult {
    /// The file as asked, without a leading `./`.
    file: String,
    /// Always `count`.
    mode: &'static str,
    /// The history read, or null without one.
    scan: Option<CountScanOut>,
    /// What the counts are over.
    scope: Scope,
    /// Most shared commits first, then by path.
    partners: Vec<CountOut>,
    /// True when more files shared a commit than `partners` holds.
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

impl Shed for CountResult {
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

/// The history a count was read from: HEAD at the read, the stored
/// window, every commit the window walked (merges included), and how many
/// were skipped as bulk.
#[derive(Clone, Serialize)]
struct CountScanOut {
    head: String,
    window: WindowOut,
    commits_seen: u64,
    skipped_bulk: u64,
}

/// One partner and the kept commits it shared with the asked file.
#[derive(Clone, Serialize)]
struct CountOut {
    path: String,
    count: u64,
}

/// The raw co-change counts of `request.file`, at most `request.limit`,
/// fitted to `request.max_tokens`.
pub(crate) fn cochange_count(request: &CountRequest) -> Result<Reply<CountResult>, Failure> {
    check_limit(request.limit)?;
    budget::check(request.max_tokens)?;
    let store = current_store(&request.repo)?;
    let file = relative(&request.file).to_owned();
    let (scan, partners, truncated) = counts(&store, request, &file)?;
    let result = CountResult {
        file,
        mode: "count",
        scan,
        scope: Scope::count(),
        partners,
        truncated,
        tokens: None,
    };
    answer(result, &store, &request.repo, request.max_tokens)
}

/// What `counts` reads: the history, its first `limit` partners, and
/// whether there were more.
type Counted = (Option<CountScanOut>, Vec<CountOut>, bool);

/// `file`'s counts over the stored scan's window, or nothing without a
/// scan or a git root.
fn counts(store: &Store, request: &CountRequest, file: &str) -> Result<Counted, Failure> {
    let Some((_, window)) = stored_scan(store)? else {
        return Ok((None, Vec::new(), false));
    };
    let root = canonical_repo(&request.repo)?;
    let Some(counts) = (match phantom_fractal_git::cochange_counts(&root, &window, file) {
        Ok(counts) => counts,
        Err(phantom_fractal_git::GitError::NotTracked { .. }) => {
            return Err(Failure::new(
                ErrorKind::NotFound,
                "file is not tracked at HEAD; git log has its history",
            ));
        }
        Err(err) => return Err(history_failure(err)),
    }) else {
        return Ok((None, Vec::new(), false));
    };
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let truncated = counts.partners.len() > limit;
    let partners = counts
        .partners
        .into_iter()
        .take(limit)
        .map(|partner| CountOut {
            path: partner.path,
            count: partner.count,
        })
        .collect();
    let scan = CountScanOut {
        head: counts.head,
        window: window.into(),
        commits_seen: counts.commits_seen,
        skipped_bulk: counts.skipped_bulk,
    };
    Ok((Some(scan), partners, truncated))
}

/// A git error's text may hold a path, so it is not passed on.
fn history_failure(_: phantom_fractal_git::GitError) -> Failure {
    Failure::new(
        ErrorKind::Usage,
        "git history could not be read; no co-change count was made",
    )
}
