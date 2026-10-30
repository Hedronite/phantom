//! Phantom command line. Builds as both `phantom-fractal` and `fract`.
//!
//! `index PATH` stores the Rust files under PATH in a database in the data
//! directory and resolves their calls. `find NAME` looks symbols up in that
//! database and returns at most 100. `callers SYMBOL` and `callees SYMBOL`
//! return the stored call edges of one symbol, at most 100. `impact SYMBOL`
//! walks those edges backwards to say what a change to the symbol reaches.
//! `show SYMBOL` gives one symbol's detail, members, and a capped look at
//! its edges, with its source under `--source`. `rank` lists the
//! best-ranked symbols or files, and `map` packs a ranked repo map into a
//! token budget. On a git root `index` also stores the co-change of the
//! history, and `cochange FILE` lists the files that change with FILE.
//! `--max-tokens` fits a read reply to a budget too. `status` says whether
//! the index is stale and why. `version` says what this binary is and
//! needs no index. Each prints the JSON envelope from
//! `phantom-fractal-proto`, whose `meta` says, on every success that read
//! an index, whether that index is stale. `index` holds the database's
//! writer lock while it runs. `mcp` serves the same
//! operations to an MCP client over stdin and stdout, for the trees named
//! by `--root` only. There are no other commands.
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
use phantom_fractal_git::Window;
use serde::Serialize;

use crate::ops::{
    CallsRequest, CochangeRequest, Failure, FindRequest, ImpactRequest, IndexRequest, MapRequest,
    RankKind, RankRequest, Reply, ShowRequest, StatusRequest, Target, VersionRequest,
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
    /// directory, never under PATH. On a git root, also store the co-change
    /// of its history, read without running git.
    Index {
        path: PathBuf,
        /// Days of history before HEAD's commit the co-change scan reads,
        /// 1 to 3650.
        #[arg(long, default_value_t = Window::DEFAULT_DAYS,
              value_parser = clap::value_parser!(u32).range(1..=i64::from(Window::MAX_DAYS)))]
        cochange_days: u32,
        /// Most commits the co-change scan reads, 1 to 50000.
        #[arg(long, default_value_t = Window::DEFAULT_COMMITS,
              value_parser = clap::value_parser!(u32).range(1..=i64::from(Window::MAX_COMMITS)))]
        cochange_commits: u32,
    },
    /// Find indexed symbols by name.
    Find {
        query: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
    },
    /// List the resolved calls to one symbol, by bare or qualified name.
    Callers {
        symbol: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
    },
    /// List the resolved calls one symbol makes, by bare or qualified name.
    Callees {
        symbol: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
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
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
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
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
    },
    /// List the best-ranked symbols, or files, of the index.
    Rank {
        /// Rank files instead of symbols.
        #[arg(long, conflicts_with = "symbols")]
        files: bool,
        /// Rank symbols, the default.
        #[arg(long)]
        symbols: bool,
        /// An indexed file, relative to the repo, to personalise the rank
        /// to. Repeatable, at most 100 times.
        #[arg(long = "focus", value_name = "PATH")]
        focus: Vec<String>,
        /// Most hits returned, 1 to 100.
        #[arg(long, default_value_t = ops::rank::DEFAULT_LIMIT,
              value_parser = clap::value_parser!(u64).range(ops::rank::LIMIT_MIN..=ops::rank::LIMIT_MAX))]
        limit: u64,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
    },
    /// List the files that tend to change with FILE, from the co-change
    /// `index` stored on a git root.
    Cochange {
        /// An indexed file, relative to the repo.
        file: String,
        /// Lowest lift listed, a number at least 0.
        #[arg(long, default_value_t = ops::cochange::DEFAULT_MIN_LIFT, value_parser = min_lift)]
        min_lift: f64,
        /// Most partners returned, 1 to 100.
        #[arg(long, default_value_t = ops::cochange::DEFAULT_LIMIT,
              value_parser = clap::value_parser!(u64).range(ops::cochange::LIMIT_MIN..=ops::cochange::LIMIT_MAX))]
        limit: u64,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Most estimated tokens the reply may cost, 1 to 100000. List
        /// entries are dropped in a fixed order to fit, and `data.tokens`
        /// says how many.
        #[arg(long, value_parser = max_tokens_range())]
        max_tokens: Option<u64>,
    },
    /// Pack a ranked map of the repo into a budget of estimated tokens.
    Map {
        /// The most estimated tokens the map may cost, 1 to 100000.
        #[arg(long,
              value_parser = clap::value_parser!(u64).range(ops::map::BUDGET_MIN..=ops::map::BUDGET_MAX))]
        budget: u64,
        /// An indexed file, relative to the repo, to put first. Repeatable,
        /// at most 100 times.
        #[arg(long = "focus", value_name = "PATH")]
        focus: Vec<String>,
        /// How much of each file to print.
        #[arg(long, default_value = "symbols",
              value_parser = ["files", "symbols", "full"])]
        detail: String,
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Say whether the index is stale and why: what an index pass would
    /// change, HEAD now and at the last pass, the stored counts, versions
    /// and epochs, and whether a writer holds the index. Writes nothing.
    Status {
        /// Tree that was indexed. Defaults to the current directory.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Print the name, version, and commit of this binary. Needs no index.
    Version,
    /// Serve the read operations to an MCP client: JSON-RPC on stdin and
    /// stdout until stdin closes.
    Mcp {
        /// Also offer the `index_repo` tool, which writes the data directory.
        #[arg(long)]
        allow_index: bool,
        /// A tree the tools may read, and `index_repo` may index.
        /// Repeatable. Defaults to the current directory.
        #[arg(long = "root", value_name = "PATH")]
        root: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return clap_exit(&err),
    };
    match cli.command {
        Command::Index {
            path,
            cochange_days,
            cochange_commits,
        } => emit(ops::index(&IndexRequest {
            path,
            window: Window {
                days: cochange_days,
                max_commits: cochange_commits,
            },
        })),
        Command::Find {
            query,
            repo,
            max_tokens,
        } => emit(ops::find(&FindRequest {
            query,
            repo,
            max_tokens,
        })),
        Command::Callers {
            symbol,
            repo,
            max_tokens,
        } => emit(ops::callers(&CallsRequest {
            symbol,
            repo,
            max_tokens,
        })),
        Command::Callees {
            symbol,
            repo,
            max_tokens,
        } => emit(ops::callees(&CallsRequest {
            symbol,
            repo,
            max_tokens,
        })),
        Command::Impact {
            symbol,
            repo,
            depth,
            min_confidence,
            no_tests,
            summary,
            max_tokens,
        } => emit(ops::impact(&ImpactRequest {
            symbol,
            repo,
            options: ImpactOptions {
                max_depth: depth,
                min_confidence,
                include_tests: !no_tests,
            },
            summary,
            max_tokens,
        })),
        Command::Show {
            symbol,
            id,
            repo,
            source,
            max_tokens,
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
                max_tokens,
            }))
        }
        Command::Rank {
            files,
            symbols: _,
            focus,
            limit,
            repo,
            max_tokens,
        } => emit(ops::rank(&RankRequest {
            kind: if files {
                RankKind::Files
            } else {
                RankKind::Symbols
            },
            focus,
            limit,
            repo,
            max_tokens,
        })),
        Command::Cochange {
            file,
            min_lift,
            limit,
            repo,
            max_tokens,
        } => emit(ops::cochange(&CochangeRequest {
            file,
            min_lift,
            limit,
            repo,
            max_tokens,
        })),
        Command::Map {
            budget,
            focus,
            detail,
            repo,
        } => emit(ops::map(&MapRequest {
            budget,
            focus,
            // clap admits only the three names.
            detail: ops::map::detail(&detail).unwrap_or_default(),
            repo,
        })),
        Command::Status { repo } => emit(ops::status(&StatusRequest { repo })),
        Command::Version => emit(ops::version(&VersionRequest)),
        Command::Mcp { allow_index, root } => mcp::serve(allow_index, &root),
    }
}

/// `--max-tokens`: an integer from 1 to 100000.
fn max_tokens_range() -> clap::builder::RangedU64ValueParser<u64> {
    clap::value_parser!(u64).range(ops::budget::MAX_TOKENS_MIN..=ops::budget::MAX_TOKENS_MAX)
}

/// `--min-lift`: a finite number, at least 0.
fn min_lift(text: &str) -> Result<f64, String> {
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{text} is not a number"))?;
    if ops::cochange::min_lift_in_range(value) {
        Ok(value)
    } else {
        Err(format!("{text} is not a finite number at least 0"))
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
fn emit<T: Serialize>(result: Result<Reply<T>, Failure>) -> ExitCode {
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
