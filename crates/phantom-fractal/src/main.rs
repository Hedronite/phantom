//! Phantom command line. Builds as both `phantom-fractal` and `fract`.
//!
//! `index PATH` stores the Rust files under PATH in a database in the data
//! directory and resolves their calls. `find NAME` looks symbols up in that
//! database and returns at most 100. `callers SYMBOL` and `callees SYMBOL`
//! return the stored call edges of one symbol, at most 100. `impact SYMBOL`
//! walks those edges backwards to say what a change to the symbol reaches.
//! `show SYMBOL` gives one symbol's detail, members, and a capped look at
//! its edges, with its source under `--source`. `version` says what this
//! binary is and needs no index. Each prints the JSON envelope from
//! `phantom-fractal-proto`. There are no other commands.
//!
//! This file only parses the arguments, calls one operation from `ops`,
//! and prints its envelope.

#![forbid(unsafe_code)]

mod mcp;
mod ops;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use phantom_fractal_analyze::ImpactOptions;
use serde::Serialize;

use crate::ops::{
    CallsRequest, Failure, FindRequest, ImpactRequest, IndexRequest, ShowRequest, Target,
    VersionRequest,
};

/// Canon exit 1: usage or internal error.
const EXIT_ERROR: u8 = 1;

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
    /// Output is always the JSON envelope. Accepted anywhere, so a caller
    /// that asks for JSON is not refused; it changes nothing.
    #[arg(long, global = true)]
    json: bool,
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
    /// Walk the calls into one symbol backwards: what a change to it reaches.
    Impact {
        symbol: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Deepest level reported, 1 to 5.
        #[arg(long, default_value_t = ImpactOptions::default().max_depth,
              value_parser = clap::value_parser!(u8).range(1..=i64::from(ImpactOptions::MAX_DEPTH)))]
        depth: u8,
        /// Lowest path confidence reported, above 0 and at most 1.
        #[arg(long, default_value_t = ImpactOptions::default().min_confidence,
              value_parser = min_confidence)]
        min_confidence: f64,
        /// Leave tests out of the answer.
        #[arg(long)]
        no_tests: bool,
        /// Counts, reasons, and risk only, without the hits.
        #[arg(long)]
        summary: bool,
    },
    /// Show one symbol's detail, members, callers, and callees, by name or
    /// by stable key.
    Show {
        /// A bare or qualified name, as `callers` takes. Give this or
        /// `--id`, not both.
        #[arg(required_unless_present = "id", conflicts_with = "id")]
        symbol: Option<String>,
        /// The symbol's exact stable key.
        #[arg(long)]
        id: Option<String>,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Also return the symbol's source lines.
        #[arg(long)]
        source: bool,
    },
    /// Print the name, version, and commit of this binary. Needs no index.
    Version,
    /// Serve the read operations to an MCP client: JSON-RPC on stdin and
    /// stdout until stdin closes.
    Mcp {
        /// Also offer the `index_repo` tool, which writes the data directory.
        #[arg(long)]
        allow_index: bool,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return clap_exit(&err),
    };
    match cli.command {
        Command::Index { path } => emit(ops::index(&IndexRequest { path })),
        Command::Find { query, repo } => emit(ops::find(&FindRequest { query, repo })),
        Command::Callers { symbol, repo } => emit(ops::callers(&CallsRequest { symbol, repo })),
        Command::Callees { symbol, repo } => emit(ops::callees(&CallsRequest { symbol, repo })),
        Command::Impact {
            symbol,
            repo,
            depth,
            min_confidence,
            no_tests,
            summary,
        } => emit(ops::impact(&ImpactRequest {
            symbol,
            repo,
            options: ImpactOptions {
                max_depth: depth,
                min_confidence,
                include_tests: !no_tests,
            },
            summary,
        })),
        Command::Show {
            symbol,
            id,
            repo,
            source,
        } => {
            // clap admits exactly one of SYMBOL and --id.
            let target = match id {
                Some(key) => Target::Id(key),
                None => Target::Name(symbol.unwrap_or_default()),
            };
            emit(ops::show(&ShowRequest {
                target,
                repo,
                source,
            }))
        }
        Command::Version => emit(ops::version(&VersionRequest)),
        Command::Mcp { allow_index } => mcp::serve(allow_index),
    }
}

/// `--min-confidence`: a number above 0 and at most 1. NaN is refused.
fn min_confidence(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{text} is not a number"))?;
    let options = ImpactOptions {
        min_confidence: value,
        ..ImpactOptions::default()
    };
    if options.in_range() {
        Ok(value)
    } else {
        Err(format!("{text} is not above 0 and at most 1"))
    }
}

/// Prints the envelope for `result` and exits with its kind's code.
fn emit<T: Serialize>(result: Result<T, Failure>) -> ExitCode {
    let (json, code) = ops::envelope(result);
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
