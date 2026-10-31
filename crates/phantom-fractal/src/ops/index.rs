//! `index`: store the Rust files under a tree, resolve their calls,
//! store the rank of the graph they make, and, on a git root, store the
//! co-change of its history (SPEC-w8 G3a). The whole pass holds the
//! database's writer lock, and it ends by storing the canonical root and
//! HEAD (SPEC-w9 G0c).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use phantom_fractal_extract::{GraphReport, IndexReport, ReasonCounts};
use phantom_fractal_git::Window;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{CochangeScan, Store, WriterLock, index_database_path};
use serde::Serialize;

use super::cochange::stored_window;
use super::{Failure, Reply, fresh};

/// What `index` is asked for.
pub(crate) struct IndexRequest {
    /// The tree to index. The database goes in the data directory, never
    /// under it.
    pub(crate) path: PathBuf,
    /// How much git history the co-change scan reads. Out of range is
    /// `Usage`.
    pub(crate) window: Window,
}

/// `data` of a successful `index`. It holds counts only, never a path.
/// The graph counts describe the stored crate graph and edges.
#[derive(Serialize)]
pub(crate) struct IndexSummary {
    written: usize,
    unchanged: usize,
    not_utf8: usize,
    skipped: usize,
    modules: usize,
    orphans: usize,
    edges: u64,
    unresolved: u64,
    external: u64,
    no_source: u64,
    method_calls: u64,
    macros: u64,
    edges_by_reason: EdgesByReason,
    /// The stored co-change scan, on a git root only.
    #[serde(skip_serializing_if = "Option::is_none")]
    cochange: Option<CochangeSummary>,
}

/// `data.cochange` of `index` and `status`: the scan's HEAD, what its
/// window walked, and how many pairs it stored.
#[derive(Serialize)]
pub(crate) struct CochangeSummary {
    head: String,
    commits_seen: u64,
    skipped_bulk: u64,
    pairs: usize,
    window: WindowOut,
}

/// A window as `index` and `cochange` print it.
#[derive(Clone, Copy, Serialize)]
pub(crate) struct WindowOut {
    days: u32,
    max_commits: u32,
}

impl From<Window> for WindowOut {
    fn from(window: Window) -> Self {
        Self {
            days: window.days,
            max_commits: window.max_commits,
        }
    }
}

/// The stored edges for each reason, keyed by the reason text that
/// `callers` and `callees` print. Every key is present, zero included.
#[derive(Serialize)]
struct EdgesByReason {
    #[serde(rename = "path-exact")]
    path_exact: u64,
    local: u64,
    #[serde(rename = "use-alias")]
    use_alias: u64,
    assoc: u64,
    #[serde(rename = "self-method")]
    self_method: u64,
    #[serde(rename = "self-assoc")]
    self_assoc: u64,
    #[serde(rename = "crate-path")]
    crate_path: u64,
    #[serde(rename = "typed-method")]
    typed_method: u64,
}

impl From<ReasonCounts> for EdgesByReason {
    fn from(counts: ReasonCounts) -> Self {
        Self {
            path_exact: counts.path_exact,
            local: counts.local,
            use_alias: counts.use_alias,
            assoc: counts.assoc,
            self_method: counts.self_method,
            self_assoc: counts.self_assoc,
            crate_path: counts.crate_path,
            typed_method: counts.typed_method,
        }
    }
}

impl IndexSummary {
    fn new(report: &IndexReport, graph: &GraphReport) -> Self {
        let counts = &graph.counts;
        Self {
            written: report.written.len(),
            unchanged: report.unchanged.len(),
            not_utf8: report.not_utf8.len(),
            skipped: report.skipped.len(),
            modules: graph.modules,
            orphans: graph.orphans,
            edges: counts.edges,
            unresolved: counts.unresolved,
            external: counts.external,
            no_source: counts.no_source,
            method_calls: counts.method_calls,
            macros: counts.macros,
            edges_by_reason: graph.edges_by_reason.into(),
            cochange: None,
        }
    }
}

/// Indexes the Rust files under `request.path` into its database in the
/// data directory, holding the database's writer lock throughout. Another
/// writer holding it is `LockHeld`.
pub(crate) fn index(request: &IndexRequest) -> Result<Reply<IndexSummary>, Failure> {
    if !request.window.in_range() {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!(
                "cochange_days must be 1 to {} and cochange_commits 1 to {}",
                Window::MAX_DAYS,
                Window::MAX_COMMITS
            ),
        ));
    }
    let root = checked_root(&request.path)?;
    let language = RustLanguage::new()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    let database = index_database_path(&root);
    // The lock comes first: it makes and checks the directory chain, so
    // even a first index creates its database under the lock, and a
    // second writer waits for it. The store is declared after the lock,
    // so it is closed before the lock is released.
    let _lock = WriterLock::acquire(&database)?;
    let mut store = Store::open(&database)?;
    let pass = pass(&root, &mut store, &language, request.window)?;
    let meta = fresh::meta(&root, &store)?;
    let summary = IndexSummary {
        cochange: pass.cochange,
        ..IndexSummary::new(&pass.report, &pass.graph)
    };
    Ok(Reply::with_meta(summary, meta))
}

/// `path` canonicalized, when it is a directory to index: `NotFound` when
/// it does not exist, and `Usage` when it is not a directory or holds the
/// data directory.
pub(crate) fn checked_root(path: &Path) -> Result<PathBuf, Failure> {
    let root = path
        .canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "index path not found"))?;
    if !root.is_dir() {
        return Err(Failure::new(
            ErrorKind::Usage,
            "index path is not a directory",
        ));
    }
    if resolve(&index_database_path(&root)).starts_with(&root) {
        return Err(Failure::new(
            ErrorKind::Usage,
            "data directory is inside the index path",
        ));
    }
    Ok(root)
}

/// What one index pass did: the files it wrote and skipped, the graph it
/// stored or kept, and the co-change scan on a git root.
pub(crate) struct Pass {
    pub(crate) report: IndexReport,
    pub(crate) graph: GraphReport,
    pub(crate) cochange: Option<CochangeSummary>,
}

/// One index pass over the canonical `root` into `store`, whose writer
/// lock the caller holds: the files, the graph, the rank, the co-change
/// scan over `window`, and then `root` and `index_head`, each written only
/// when it changed.
pub(crate) fn pass(
    root: &Path,
    store: &mut Store,
    language: &RustLanguage,
    window: Window,
) -> Result<Pass, Failure> {
    let report = phantom_fractal_extract::index(root, store, language)?;
    let graph = match report.graph {
        Some(graph) => graph,
        None => stored_graph(store, root)?,
    };
    store_rank(store)?;
    let head = phantom_fractal_git::head(root).map_err(history_failure)?;
    let cochange = store_cochange(store, root, head.as_deref(), window)?;
    // Each writes only when its value changed.
    store.set_root(&root.to_string_lossy())?;
    store.set_index_head(head.as_deref())?;
    Ok(Pass {
        report,
        graph,
        cochange,
    })
}

/// The store at `database` under its writer lock, the lock taken first,
/// as `index` takes it: acquiring it makes and checks the directory
/// chain, so even a first index creates its database under the lock, and
/// a second writer waits for it.
pub(crate) fn open_locked(database: &Path) -> Result<(Store, WriterLock), Failure> {
    let lock = WriterLock::acquire(database)?;
    let store = Store::open(database)?;
    Ok((store, lock))
}

/// Mines the co-change of the git root `root`, whose HEAD is `head`, and
/// stores it, only when no scan is stored or the stored one has another
/// HEAD or window. A root that is not the top of a git work tree has no
/// HEAD, is not mined, and gives none; a
/// scan stored while it was one is replaced, once, by an empty scan whose
/// HEAD is empty, so no read reports history the root no longer has. The
/// store keeps `meta cochange_head` as it was for a scan with no HEAD at
/// all, so the empty text is what clears it.
fn store_cochange(
    store: &mut Store,
    root: &Path,
    head: Option<&str>,
    window: Window,
) -> Result<Option<CochangeSummary>, Failure> {
    let Some(head) = head else {
        if store.cochange_head()?.is_some() {
            store.write_cochange(&CochangeScan {
                last_oid: Some(String::new()),
                ..CochangeScan::default()
            })?;
        }
        return Ok(None);
    };
    let window_json = window.to_json();
    let current = store
        .cochange_scan()?
        .filter(|scan| scan.last_oid.as_deref() == Some(head) && scan.window_json == window_json);
    let scan = match current {
        Some(scan) => scan,
        None => {
            let indexed: BTreeSet<String> =
                store.files()?.into_iter().map(|file| file.path).collect();
            let Some(scan) =
                phantom_fractal_git::mine(root, &window, &indexed).map_err(history_failure)?
            else {
                return Ok(None);
            };
            store.write_cochange(&scan)?;
            // Read back, so this pass reports what the next one will.
            store.cochange_scan()?.unwrap_or(scan)
        }
    };
    Ok(Some(summary(&scan, head.to_owned(), window)))
}

/// The stored scan's summary, as the pass that stored it printed it, or
/// none without a scan or with a cleared one.
pub(crate) fn stored_cochange(store: &Store) -> Result<Option<CochangeSummary>, Failure> {
    let Some(scan) = store.cochange_scan()? else {
        return Ok(None);
    };
    let Some(head) = scan.last_oid.clone().filter(|head| !head.is_empty()) else {
        return Ok(None);
    };
    let window = stored_window(&scan.window_json)?;
    Ok(Some(summary(&scan, head, window)))
}

fn summary(scan: &CochangeScan, head: String, window: Window) -> CochangeSummary {
    CochangeSummary {
        head,
        commits_seen: scan.commits_seen,
        skipped_bulk: scan.skipped_bulk,
        pairs: scan.pairs.len(),
        window: window.into(),
    }
}

/// A git error's text may hold a path, so it is not passed on.
fn history_failure(_: phantom_fractal_git::GitError) -> Failure {
    Failure::new(
        ErrorKind::Usage,
        "git history could not be read; the files were indexed, the co-change was not",
    )
}

/// Computes the rank and stores it, only when the stored rank is not the
/// current graph's. A pass that rebuilt nothing leaves the epochs equal
/// and writes nothing.
fn store_rank(store: &mut Store) -> Result<(), Failure> {
    if store.rank_epoch()? == store.graph_epoch()? {
        return Ok(());
    }
    let ranks = phantom_fractal_analyze::rank(&*store)?;
    store.write_rank(ranks.epoch, &ranks.symbols, &ranks.files)?;
    Ok(())
}

/// The graph counts of an index pass that rebuilt nothing, read from the
/// stored graph without writing. The resolver is pure, so its counts over
/// the stored input are the ones the last rebuild found.
fn stored_graph(store: &Store, root: &Path) -> Result<GraphReport, Failure> {
    Ok(phantom_fractal_extract::stored_graph(store, root)?)
}

/// `path` made absolute, with its longest existing prefix canonicalized, so
/// a database that does not exist yet compares with a canonical root.
fn resolve(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    for ancestor in path.ancestors() {
        if let Ok(real) = ancestor.canonicalize() {
            let rest = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
            return real.join(rest);
        }
    }
    path
}
