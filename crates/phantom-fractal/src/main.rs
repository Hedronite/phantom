//! Phantom command line. Builds as both `phantom-fractal` and `fract`.
//!
//! `index PATH` stores the Rust files under PATH in a database in the data
//! directory. `find NAME` looks symbols up in that database and returns at
//! most 100. Both print the JSON envelope from `phantom-fractal-proto`.
//! There are no other commands.

#![forbid(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use phantom_fractal_extract::{IndexError, IndexReport};
use phantom_fractal_lang::{Item, Visibility};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::{Envelope, ErrorKind};
use phantom_fractal_store::{Store, StoreError, index_database_path};
use serde::Serialize;

/// Canon exit 1: usage or internal error.
const EXIT_ERROR: u8 = 1;

/// Most hits one `find` returns.
const FIND_CAP: usize = 100;

/// Both binary names print the same text, so the name is fixed here
/// rather than taken from argv[0].
#[derive(Parser)]
#[command(
    name = "phantom-fractal",
    bin_name = "phantom-fractal",
    version,
    help_template = "{name} {version}",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
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
}

/// `data` of a successful `index`. It holds counts only, never a path.
#[derive(Serialize)]
struct IndexSummary {
    written: usize,
    unchanged: usize,
    not_utf8: usize,
    skipped: usize,
}

impl IndexSummary {
    fn new(report: &IndexReport) -> Self {
        Self {
            written: report.written.len(),
            unchanged: report.unchanged.len(),
            not_utf8: report.not_utf8.len(),
            skipped: report.skipped.len(),
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

/// An envelope error. `message` never holds an absolute path.
struct Failure {
    kind: ErrorKind,
    message: String,
}

impl Failure {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
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
        None => print_version(),
        Some(Command::Index { path }) => emit(index(&path)),
        Some(Command::Find { query, repo }) => emit(find(&query, &repo)),
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
    Ok(IndexSummary::new(&report))
}

/// Indexed symbols under `repo` named exactly `query`, ordered by path and
/// then line, at most [`FIND_CAP`] of them.
///
/// The store's bound FTS match answers an absent name without a walk.
/// Otherwise each `.rs` file the walk keeps is read back from the store.
fn find(query: &str, repo: &Path) -> Result<FindResult, Failure> {
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
    let store = Store::open(&database)?;
    if query.is_empty() || store.match_count(query)? == 0 {
        return Ok(FindResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
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
    let (envelope, code) = match result {
        Ok(data) => (Envelope::success(data), 0),
        Err(Failure { kind, message }) => (Envelope::failure(kind, message), kind.exit_code()),
    };
    let Ok(json) = serde_json::to_string(&envelope) else {
        return ExitCode::from(EXIT_ERROR);
    };
    if writeln!(std::io::stdout().lock(), "{json}").is_err() {
        return ExitCode::from(EXIT_ERROR);
    }
    ExitCode::from(u8::try_from(code).unwrap_or(EXIT_ERROR))
}

fn print_version() -> ExitCode {
    match Cli::command().print_help() {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EXIT_ERROR),
    }
}

/// Help and version exit 0. Usage errors exit 1, not clap's 2.
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
