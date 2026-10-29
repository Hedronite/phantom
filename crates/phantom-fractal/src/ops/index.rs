//! `index`: store the Rust files under a tree and resolve their calls.

use std::path::{Path, PathBuf};

use phantom_fractal_extract::{GraphReport, IndexReport, ReasonCounts};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{Store, index_database_path};
use serde::Serialize;

use super::Failure;

/// What `index` is asked for.
pub(crate) struct IndexRequest {
    /// The tree to index. The database goes in the data directory, never
    /// under it.
    pub(crate) path: PathBuf,
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
        }
    }
}

/// Indexes the Rust files under `request.path` into its database in the
/// data directory.
pub(crate) fn index(request: &IndexRequest) -> Result<IndexSummary, Failure> {
    let root = request
        .path
        .canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "index path not found"))?;
    if !root.is_dir() {
        return Err(Failure::new(
            ErrorKind::Usage,
            "index path is not a directory",
        ));
    }
    let database = index_database_path(&root);
    if resolve(&database).starts_with(&root) {
        return Err(Failure::new(
            ErrorKind::Usage,
            "data directory is inside the index path",
        ));
    }
    let language = RustLanguage::new()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    let mut store = Store::open(&database)?;
    let report = phantom_fractal_extract::index(&root, &mut store, &language)?;
    let graph = match report.graph {
        Some(graph) => graph,
        None => stored_graph(&store, &root)?,
    };
    Ok(IndexSummary::new(&report, &graph))
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
