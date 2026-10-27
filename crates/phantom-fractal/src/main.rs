//! Phantom command line. Builds as both `phantom-fractal` and `fract`.
//!
//! `index PATH` stores the Rust files under PATH in a database in the data
//! directory and resolves their calls. `find NAME` looks symbols up in that
//! database and returns at most 100. `callers SYMBOL` and `callees SYMBOL`
//! return the stored call edges of one symbol, at most 100. Each prints the
//! JSON envelope from `phantom-fractal-proto`. There are no other commands.

#![forbid(unsafe_code)]

mod calls;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use phantom_fractal_extract::{GraphReport, IndexError, IndexReport, ReasonCounts};
use phantom_fractal_lang::{Item, Visibility};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::{Envelope, ErrorKind};
use phantom_fractal_store::{Store, StoreError, index_database_path};
use serde::Serialize;

use crate::calls::{Candidates, Direction};

/// Canon exit 1: usage or internal error.
const EXIT_ERROR: u8 = 1;

/// Most hits one `find` returns.
const FIND_CAP: usize = 100;

// Both binary names print the same text, so the name is fixed here
// rather than taken from argv[0].
#[derive(Parser)]
#[command(
    name = "phantom-fractal",
    bin_name = "phantom-fractal",
    version,
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index the Rust files under PATH. The database is written to the data
    /// directory, never under PATH.
    Index { path: PathBuf },
    /// Find indexed symbols by name.
    Find {
        query: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// List the resolved calls to one symbol, by bare or qualified name.
    Callers {
        symbol: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// List the resolved calls one symbol makes, by bare or qualified name.
    Callees {
        symbol: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

/// `data` of a successful `index`. It holds counts only, never a path.
/// The graph counts describe the stored crate graph and edges.
#[derive(Serialize)]
struct IndexSummary {
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

/// `data` of a successful `find`. `truncated` is true when more than
/// [`FIND_CAP`] symbols matched and only the first are in `hits`.
#[derive(Serialize)]
struct FindResult {
    hits: Vec<SymbolHit>,
    truncated: bool,
}

/// One `find` result. `path` is relative to the indexed tree.
#[derive(Serialize)]
struct SymbolHit {
    name: String,
    kind: &'static str,
    visibility: String,
    path: String,
    line: u32,
}

impl SymbolHit {
    fn new(path: &str, item: Item) -> Self {
        let visibility = match item.visibility {
            Visibility::Private => "private".to_owned(),
            Visibility::Public => "pub".to_owned(),
            Visibility::Crate => "pub(crate)".to_owned(),
            Visibility::Restricted(text) => text,
        };
        Self {
            name: item.name,
            kind: item.kind.as_str(),
            visibility,
            path: path.to_owned(),
            line: item.start_line,
        }
    }
}

/// An envelope error. `message` and `candidates` never hold an absolute
/// path.
struct Failure {
    kind: ErrorKind,
    message: String,
    /// The envelope's `data` for an `Ambiguous` lookup.
    candidates: Option<Candidates>,
}

impl Failure {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            candidates: None,
        }
    }

    /// `Ambiguous`, with the candidates as the envelope's `data`.
    fn ambiguous(message: impl Into<String>, candidates: Candidates) -> Self {
        Self {
            candidates: Some(candidates),
            ..Self::new(ErrorKind::Ambiguous, message)
        }
    }
}

impl From<StoreError> for Failure {
    fn from(err: StoreError) -> Self {
        let kind = match err {
            StoreError::SchemaNewer { .. } => ErrorKind::SchemaNewer,
            _ => ErrorKind::Usage,
        };
        Self::new(kind, format!("store: {err}"))
    }
}

impl From<IndexError> for Failure {
    fn from(err: IndexError) -> Self {
        match err {
            IndexError::Store(err) => err.into(),
            other => Self::new(ErrorKind::Usage, other.to_string()),
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return clap_exit(&err),
    };
    match cli.command {
        Command::Index { path } => emit(index(&path)),
        Command::Find { query, repo } => emit(find(&query, &repo)),
        Command::Callers { symbol, repo } => emit(calls::calls(Direction::Callers, &symbol, &repo)),
        Command::Callees { symbol, repo } => emit(calls::calls(Direction::Callees, &symbol, &repo)),
    }
}

fn index(path: &Path) -> Result<IndexSummary, Failure> {
    let root = path
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

/// The store for the tree at `repo`, which must already be indexed. It is
/// `NotFound` when `repo` does not exist and `NoIndex` when it has no
/// database; neither case creates anything.
fn open_index(repo: &Path) -> Result<Store, Failure> {
    let root = repo
        .canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "repo path not found"))?;
    let database = index_database_path(&root);
    if !database.exists() {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "no index for this path; run index first",
        ));
    }
    Ok(Store::open(&database)?)
}

/// Indexed symbols under `repo` named exactly `query`, ordered by path and
/// then line, at most [`FIND_CAP`] of them.
///
/// The store's bound FTS match answers an absent name without a walk.
/// Otherwise each `.rs` file the walk keeps is read back from the store.
fn find(query: &str, repo: &Path) -> Result<FindResult, Failure> {
    let store = open_index(repo)?;
    if query.is_empty() || store.match_count(query)? == 0 {
        return Ok(FindResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let root = repo
        .canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "repo path not found"))?;
    let walk = phantom_fractal_extract::walk(&root)
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("walk failed: {err}")))?;
    let mut hits = Vec::new();
    for file in walk.files.iter().filter(|file| file.path.ends_with(".rs")) {
        for item in store.read_items(&file.path)? {
            if item.name == query {
                hits.push(SymbolHit::new(&file.path, item));
            }
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = hits.len() > FIND_CAP;
    hits.truncate(FIND_CAP);
    Ok(FindResult { hits, truncated })
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

/// Prints the envelope for `result` and exits with its kind's code.
fn emit<T: Serialize>(result: Result<T, Failure>) -> ExitCode {
    let (json, code) = match result {
        Ok(data) => (serde_json::to_string(&Envelope::success(data)), 0),
        Err(Failure {
            kind,
            message,
            candidates,
        }) => {
            let envelope = match candidates {
                Some(candidates) => Envelope::failure_with_data(kind, message, candidates),
                None => Envelope::<Candidates>::failure(kind, message),
            };
            (serde_json::to_string(&envelope), kind.exit_code())
        }
    };
    let Ok(json) = json else {
        return ExitCode::from(EXIT_ERROR);
    };
    if writeln!(std::io::stdout().lock(), "{json}").is_err() {
        return ExitCode::from(EXIT_ERROR);
    }
    ExitCode::from(u8::try_from(code).unwrap_or(EXIT_ERROR))
}

/// Help and version exit 0. A bare run prints help and exits 1 (usage),
/// not clap's 2. Other usage errors exit 1 as well.
fn clap_exit(err: &clap::Error) -> ExitCode {
    let code = if err.use_stderr() {
        ExitCode::from(EXIT_ERROR)
    } else {
        ExitCode::SUCCESS
    };
    // If printing fails there is no stream left to report it on.
    let _ = err.print();
    code
}
