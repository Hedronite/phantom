//! The tools: their definitions, their argument checks, and the
//! [`crate::ops`] call behind each one (SPEC-w6 G2b, SPEC-w7 G3b, SPEC-w8
//! G0b, G0c and G3b, SPEC-w9 G0c, G2b and G3b, SPEC-w10 G0b).
//!
//! A tool never does an operation's work itself. It turns its arguments
//! into the request the command line builds for the same flags, calls the
//! operation, and returns the envelope the command line prints. Its
//! `repo` is checked against the [`Roots`] last, after every other
//! argument. Every string argument has a [`max_length`], in its schema and
//! in the check, so an over-long one never reaches the store.

use std::cell::RefCell;
use std::path::PathBuf;

use phantom_fractal_analyze::{ImpactOptions, PATH_DEFAULT_DEPTH, PATH_MAX_DEPTH, PATH_MIN_DEPTH};
use phantom_fractal_git::Window;
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::describe;
use super::roots::Roots;
use crate::ops::budget::{MAX_TOKENS_MAX, MAX_TOKENS_MIN};
use crate::ops::cochange::{
    self as cochange, DEFAULT_MIN_LIFT, MIN_LIFT_MAX, min_lift_in_range, min_lift_usage,
};
use crate::ops::map::{BUDGET_MAX, BUDGET_MIN};
use crate::ops::rank::{DEFAULT_LIMIT, FOCUS_MAX, LIMIT_MAX, LIMIT_MIN, focus_cap};
use crate::ops::{
    self, CallsRequest, CochangeRequest, CountRequest, DiffRequest, Failure, FindRequest,
    ImpactRequest, IndexRequest, MapRequest, PathRequest, RankKind, RankRequest, Reply,
    ShowRequest, StatusRequest, Target,
};

/// One tool call's envelope line and whether it reports a failure.
pub(crate) struct Outcome {
    /// The envelope exactly as the command line prints it, without the
    /// newline.
    pub(crate) envelope: String,
    /// True exactly when the envelope's `ok` is false.
    pub(crate) is_error: bool,
}

const REPO: &str = "The indexed tree: one of the server's roots. Defaults to the only root, or \
    else to the server's working directory when it is a root; a relative path resolves against \
    the working directory.";

const SYMBOL: &str = "A bare name such as `parse`; `Type::method` such as `App::watch_dir`; a \
    module path such as `tui::app::App::watch_dir` or `config::parse`; or a method by its module \
    and name, such as `app::watch_dir`.";

const FOCUS: &str = "Indexed files, each relative to the repo, such as `src/lib.rs`.";

const MAX_TOKENS: &str = "The most estimated tokens the reply may cost. List entries are \
    dropped, in a fixed order, until the reply fits, and `data.tokens` gives the budget, the \
    estimate and the count dropped.";

/// The tool definitions `tools/list` returns. `index_repo` is listed only
/// when `allow_index` is set.
pub(crate) fn list(allow_index: bool) -> Vec<Value> {
    let defaults = ImpactOptions::default();
    let read = json!({ "readOnlyHint": true, "idempotentHint": true });
    let mut tools = vec![
        json!({
            "name": "find_symbols",
            "description": describe::FIND_SYMBOLS,
            "inputSchema": schema(
                json!({
                    "query": { "type": "string", "description": "The exact symbol name." },
                    "repo": { "type": "string", "description": REPO },
                    "max_tokens": max_tokens(),
                }),
                &["query"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "show_symbol",
            "description": describe::SHOW_SYMBOL,
            "inputSchema": schema(
                json!({
                    "symbol": { "type": "string", "description": SYMBOL },
                    "id": {
                        "type": "string",
                        "description": "A stable key, as `show_symbol` and ambiguous lookups \
                            return it.",
                    },
                    "repo": { "type": "string", "description": REPO },
                    "source": {
                        "type": "boolean",
                        "default": false,
                        "description": "Also return the symbol's source lines.",
                    },
                    "max_tokens": max_tokens(),
                }),
                &[],
            ),
            "annotations": read,
        }),
        json!({
            "name": "callers",
            "description": describe::CALLERS,
            "inputSchema": schema(
                json!({
                    "symbol": { "type": "string", "description": SYMBOL },
                    "repo": { "type": "string", "description": REPO },
                    "max_tokens": max_tokens(),
                }),
                &["symbol"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "callees",
            "description": describe::CALLEES,
            "inputSchema": schema(
                json!({
                    "symbol": { "type": "string", "description": SYMBOL },
                    "repo": { "type": "string", "description": REPO },
                    "max_tokens": max_tokens(),
                }),
                &["symbol"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "impact",
            "description": describe::IMPACT,
            "inputSchema": schema(
                json!({
                    "symbol": { "type": "string", "description": SYMBOL },
                    "repo": { "type": "string", "description": REPO },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": ImpactOptions::MAX_DEPTH,
                        "default": defaults.max_depth,
                        "description": "The deepest level reported.",
                    },
                    "min_confidence": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "maximum": 1,
                        "default": defaults.min_confidence,
                        "description": "The lowest path confidence reported.",
                    },
                    "include_tests": {
                        "type": "boolean",
                        "default": true,
                        "description": "List the tests that reach the symbol.",
                    },
                    "summary": {
                        "type": "boolean",
                        "default": false,
                        "description": "Counts, reasons and risk only, without the hits.",
                    },
                    "max_tokens": max_tokens(),
                }),
                &["symbol"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "top_ranked",
            "description": describe::TOP_RANKED,
            "inputSchema": schema(
                json!({
                    "kind": {
                        "type": "string",
                        "enum": ["symbols", "files"],
                        "default": "symbols",
                        "description": "Rank symbols or files.",
                    },
                    "focus": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": FOCUS_MAX,
                        "description": FOCUS,
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": LIMIT_MIN,
                        "maximum": LIMIT_MAX,
                        "default": DEFAULT_LIMIT,
                        "description": "The most hits returned.",
                    },
                    "repo": { "type": "string", "description": REPO },
                    "max_tokens": max_tokens(),
                }),
                &[],
            ),
            "annotations": read,
        }),
        json!({
            "name": "repo_map",
            "description": describe::REPO_MAP,
            "inputSchema": schema(
                json!({
                    "budget": {
                        "type": "integer",
                        "minimum": BUDGET_MIN,
                        "maximum": BUDGET_MAX,
                        "description": "The most estimated tokens the map may cost.",
                    },
                    "focus": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": FOCUS_MAX,
                        "description": FOCUS,
                    },
                    "detail": {
                        "type": "string",
                        "enum": ["files", "symbols", "full"],
                        "default": "symbols",
                        "description": "File lines only, file and symbol lines, or symbol \
                            lines with doc lines.",
                    },
                    "repo": { "type": "string", "description": REPO },
                }),
                &["budget"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "cochange",
            "description": describe::COCHANGE,
            "inputSchema": schema(
                json!({
                    "file": {
                        "type": "string",
                        "description": "An indexed file, relative to the repo, such as \
                            `src/lib.rs`.",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["decayed", "count"],
                        "default": "decayed",
                        "description": "`decayed`: the stored, decay-weighted, lift-filtered \
                            partners among indexed Rust files. `count`: for every tracked file, \
                            the commits it shared with `file`, with no decay and no lift floor.",
                    },
                    "min_lift": {
                        "type": "number",
                        "minimum": 0,
                        "maximum": MIN_LIFT_MAX,
                        "default": DEFAULT_MIN_LIFT,
                        "description": "The lowest lift listed, in mode `decayed` only.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": cochange::LIMIT_MIN,
                        "maximum": cochange::LIMIT_MAX,
                        "default": cochange::DEFAULT_LIMIT,
                        "description": "The most partners returned.",
                    },
                    "repo": { "type": "string", "description": REPO },
                    "max_tokens": max_tokens(),
                }),
                &["file"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "diff_impact",
            "description": describe::DIFF_IMPACT,
            "inputSchema": schema(
                json!({
                    "against": {
                        "type": "string",
                        "description": "The base instead of HEAD: a branch, a tag, or a commit id \
                            of at least 7 hex digits.",
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": ImpactOptions::MAX_DEPTH,
                        "default": defaults.max_depth,
                        "description": "The deepest level reported.",
                    },
                    "min_confidence": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "maximum": 1,
                        "default": defaults.min_confidence,
                        "description": "The lowest path confidence reported.",
                    },
                    "include_tests": {
                        "type": "boolean",
                        "default": true,
                        "description": "List the tests the change reaches.",
                    },
                    "summary_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "Counts, reasons and risk only, without the hits.",
                    },
                    "max_tokens": max_tokens(),
                    "repo": { "type": "string", "description": REPO },
                }),
                &[],
            ),
            "annotations": read,
        }),
        json!({
            "name": "symbol_path",
            "description": describe::SYMBOL_PATH,
            "inputSchema": schema(
                json!({
                    "from": {
                        "type": "string",
                        "description": "Where the path starts: a stable key, or a name as \
                            `show_symbol` takes it.",
                    },
                    "to": {
                        "type": "string",
                        "description": "Where the path ends, the same way.",
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": PATH_MIN_DEPTH,
                        "maximum": PATH_MAX_DEPTH,
                        "default": PATH_DEFAULT_DEPTH,
                        "description": "The most edges walked.",
                    },
                    "max_tokens": {
                        "type": "integer",
                        "minimum": MAX_TOKENS_MIN,
                        "maximum": MAX_TOKENS_MAX,
                        "description": "The most estimated tokens the reply may cost. No hop is \
                            dropped: a path that does not fit is a `budget_too_small` result.",
                    },
                    "repo": { "type": "string", "description": REPO },
                }),
                &["from", "to"],
            ),
            "annotations": read,
        }),
        json!({
            "name": "repo_status",
            "description": describe::REPO_STATUS,
            "inputSchema": schema(
                json!({
                    "repo": { "type": "string", "description": REPO },
                }),
                &[],
            ),
            "annotations": read,
        }),
        json!({
            "name": "repos",
            "description": describe::REPOS,
            "inputSchema": schema(json!({}), &[]),
            "annotations": read,
        }),
    ];
    if allow_index {
        tools.push(json!({
            "name": "index_repo",
            "description": describe::INDEX_REPO,
            "inputSchema": schema(
                json!({
                    "path": {
                        "type": "string",
                        "description": "The tree to index: one of the server's roots. A relative \
                            path resolves against the server's working directory.",
                    },
                    "cochange_days": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": Window::MAX_DAYS,
                        "default": Window::DEFAULT_DAYS,
                        "description": "Days of git history before HEAD's commit the co-change \
                            scan reads, on a git root.",
                    },
                    "cochange_commits": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": Window::MAX_COMMITS,
                        "default": Window::DEFAULT_COMMITS,
                        "description": "The most commits the co-change scan reads.",
                    },
                }),
                &["path"],
            ),
            "annotations": { "readOnlyHint": false, "destructiveHint": false },
        }));
    }
    tools
}

fn max_tokens() -> Value {
    json!({
        "type": "integer",
        "minimum": MAX_TOKENS_MIN,
        "maximum": MAX_TOKENS_MAX,
        "description": MAX_TOKENS,
    })
}

/// An input schema. Each string property, and each string item of an
/// array property, gets its [`max_length`] as `maxLength`.
fn schema(mut properties: Value, required: &[&str]) -> Value {
    if let Some(properties) = properties.as_object_mut() {
        for (name, property) in properties.iter_mut() {
            let text = if property["type"] == "array" {
                &mut property["items"]
            } else {
                property
            };
            if text["type"] == "string" {
                text["maxLength"] = json!(max_length(name));
            }
        }
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Runs tool `name`, or none when no such tool is offered. `Err` means the
/// envelope could not be encoded.
pub(crate) fn call(
    name: &str,
    arguments: &Map<String, Value>,
    allow_index: bool,
    roots: &Roots,
) -> Option<Result<Outcome, ()>> {
    let outcome = match name {
        "find_symbols" => run(find_request(arguments, roots), ops::find),
        "show_symbol" => run(show_request(arguments, roots), ops::show),
        "callers" => run(calls_request(arguments, roots), ops::callers),
        "callees" => run(calls_request(arguments, roots), ops::callees),
        "impact" => run(impact_request(arguments, roots), ops::impact),
        "top_ranked" => run(rank_request(arguments, roots), ops::rank),
        "repo_map" => run(map_request(arguments, roots), ops::map),
        "cochange" => cochange_call(arguments, roots),
        "diff_impact" => run(diff_request(arguments, roots), ops::impact_diff),
        "symbol_path" => run(path_request(arguments, roots), ops::symbol_path),
        "repo_status" => run(status_request(arguments, roots), ops::status),
        "repos" => run(repos_request(arguments, roots), |roots: &Vec<PathBuf>| {
            ops::registry::roots(roots)
        }),
        "index_repo" if allow_index => run(index_request(arguments, roots), ops::index),
        _ => return None,
    };
    Some(outcome)
}

fn find_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<FindRequest, Failure> {
    let args = Args::new(arguments, &["query", "repo", "max_tokens"])?;
    let query = args.read(args.required_string("query"));
    let repo = args.read(args.string("repo"));
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    Ok(FindRequest {
        query,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
}

fn show_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<ShowRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "id", "repo", "source", "max_tokens"])?;
    let target = args.target()?;
    let repo = args.read(args.string("repo"));
    let source = args.read(args.boolean("source")).unwrap_or(false);
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    Ok(ShowRequest {
        target,
        repo: roots.indexed(repo.as_deref())?,
        source,
        max_tokens,
    })
}

fn calls_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<CallsRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "repo", "max_tokens"])?;
    let symbol = args.read(args.required_string("symbol"));
    let repo = args.read(args.string("repo"));
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    Ok(CallsRequest {
        symbol,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
}

fn impact_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<ImpactRequest, Failure> {
    let args = Args::new(
        arguments,
        &[
            "symbol",
            "repo",
            "depth",
            "min_confidence",
            "include_tests",
            "summary",
            "max_tokens",
        ],
    )?;
    let options = args.read(args.impact_options());
    let symbol = args.read(args.required_string("symbol"));
    let repo = args.read(args.string("repo"));
    let summary = args.read(args.boolean("summary")).unwrap_or(false);
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    Ok(ImpactRequest {
        symbol,
        repo: roots.indexed(repo.as_deref())?,
        options,
        summary,
        max_tokens,
    })
}

fn rank_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<RankRequest, Failure> {
    let args = Args::new(arguments, &["kind", "focus", "limit", "repo", "max_tokens"])?;
    let kind = args
        .read(args.choice("kind", RankKind::parse, "kind must be symbols or files"))
        .unwrap_or(RankKind::Symbols);
    let focus = args.read(args.focus());
    let limit = args
        .read(args.integer("limit", LIMIT_MIN, LIMIT_MAX))
        .unwrap_or(DEFAULT_LIMIT);
    let repo = args.read(args.string("repo"));
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    Ok(RankRequest {
        kind,
        focus,
        limit,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
}

fn map_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<MapRequest, Failure> {
    let args = Args::new(arguments, &["budget", "focus", "detail", "repo"])?;
    let detail = args
        .read(args.choice(
            "detail",
            ops::map::detail,
            "detail must be files, symbols or full",
        ))
        .unwrap_or_default();
    let budget = args.read(required(
        "budget",
        args.integer("budget", BUDGET_MIN, BUDGET_MAX),
    ));
    let focus = args.read(args.focus());
    let repo = args.read(args.string("repo"));
    args.checked()?;
    Ok(MapRequest {
        budget,
        focus,
        detail,
        repo: roots.indexed(repo.as_deref())?,
    })
}

/// `cochange` in its `mode`: the stored decayed partners, or the raw
/// counts.
fn cochange_call(arguments: &Map<String, Value>, roots: &Roots) -> Result<Outcome, ()> {
    match cochange_request(arguments, roots) {
        Ok((request, true)) => run(Ok(count_request(request)), ops::cochange_count),
        other => run(other.map(|(request, _)| request), ops::cochange),
    }
}

/// The same file, limit, tree and budget, for `mode: "count"`.
fn count_request(request: CochangeRequest) -> CountRequest {
    CountRequest {
        file: request.file,
        limit: request.limit,
        repo: request.repo,
        max_tokens: request.max_tokens,
    }
}

/// The request, and whether `mode` is `count`. `min_lift` is for mode
/// `decayed` only.
fn cochange_request(
    arguments: &Map<String, Value>,
    roots: &Roots,
) -> Result<(CochangeRequest, bool), Failure> {
    let args = Args::new(
        arguments,
        &["file", "mode", "min_lift", "limit", "repo", "max_tokens"],
    )?;
    let file = args.read(args.required_string("file"));
    let count = args
        .read(args.choice("mode", count_mode, "mode must be decayed or count"))
        .unwrap_or(false);
    if count && arguments.contains_key("min_lift") {
        args.read::<()>(Err(usage("min_lift is for mode decayed only")));
    }
    let min_lift = args.read(args.min_lift()).unwrap_or(DEFAULT_MIN_LIFT);
    let limit = args
        .read(args.integer("limit", cochange::LIMIT_MIN, cochange::LIMIT_MAX))
        .unwrap_or(cochange::DEFAULT_LIMIT);
    let repo = args.read(args.string("repo"));
    let max_tokens = args.read(args.max_tokens());
    args.checked()?;
    let request = CochangeRequest {
        file,
        min_lift,
        limit,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    };
    Ok((request, count))
}

/// `mode`: true for `count`, false for `decayed`.
fn count_mode(mode: &str) -> Option<bool> {
    match mode {
        "count" => Some(true),
        "decayed" => Some(false),
        _ => None,
    }
}

fn diff_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<DiffRequest, Failure> {
    let args = Args::new(
        arguments,
        &[
            "against",
            "depth",
            "min_confidence",
            "include_tests",
            "summary_only",
            "max_tokens",
            "repo",
        ],
    )?;
    let options = args.read(args.impact_options());
    let against = args.read(args.string("against"));
    let summary = args.read(args.boolean("summary_only")).unwrap_or(false);
    let max_tokens = args.read(args.max_tokens());
    let repo = args.read(args.string("repo"));
    args.checked()?;
    Ok(DiffRequest {
        against,
        repo: roots.indexed(repo.as_deref())?,
        options,
        summary,
        max_tokens,
    })
}

fn path_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<PathRequest, Failure> {
    let args = Args::new(
        arguments,
        &["from", "to", "max_depth", "max_tokens", "repo"],
    )?;
    let from = args.read(args.required_string("from"));
    let to = args.read(args.required_string("to"));
    let max_depth = args
        .read(args.integer(
            "max_depth",
            u64::from(PATH_MIN_DEPTH),
            u64::from(PATH_MAX_DEPTH),
        ))
        // The range above fits in u8.
        .map_or(PATH_DEFAULT_DEPTH, |depth| depth as u8);
    let max_tokens = args.read(args.max_tokens());
    let repo = args.read(args.string("repo"));
    args.checked()?;
    Ok(PathRequest {
        from,
        to,
        max_depth,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
}

/// `repos` takes no argument and lists the indexed roots.
fn repos_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<Vec<PathBuf>, Failure> {
    if !arguments.is_empty() {
        return Err(usage("this tool takes no arguments"));
    }
    Ok(roots.all().to_vec())
}

fn status_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<StatusRequest, Failure> {
    let args = Args::new(arguments, &["repo"])?;
    let repo = args.string("repo")?;
    Ok(StatusRequest {
        repo: roots.indexed(repo.as_deref())?,
    })
}

/// `index_repo` takes a root whether or not it has an index yet.
fn index_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<IndexRequest, Failure> {
    let args = Args::new(arguments, &["path", "cochange_days", "cochange_commits"])?;
    let path = args.read(args.required_string("path"));
    let days = args.read(args.integer("cochange_days", 1, u64::from(Window::MAX_DAYS)));
    let commits = args.read(args.integer("cochange_commits", 1, u64::from(Window::MAX_COMMITS)));
    args.checked()?;
    // The ranges above fit in u32.
    let window = Window {
        days: days.map_or(Window::DEFAULT_DAYS, |days| days as u32),
        max_commits: commits.map_or(Window::DEFAULT_COMMITS, |commits| commits as u32),
    };
    Ok(IndexRequest {
        path: roots.root(Some(&path))?,
        window,
    })
}

/// Calls `op` on a well-formed request, or returns the usage envelope of a
/// bad one.
fn run<R, T: Serialize>(
    request: Result<R, Failure>,
    op: impl FnOnce(&R) -> Result<Reply<T>, Failure>,
) -> Result<Outcome, ()> {
    let (envelope, code) = match request {
        Ok(request) => ops::envelope(op(&request)),
        Err(failure) => ops::envelope::<()>(Err(failure)),
    };
    Ok(Outcome {
        envelope: envelope.map_err(|_| ())?,
        is_error: code != 0,
    })
}

fn usage(message: &str) -> Failure {
    Failure::new(ErrorKind::Usage, message)
}

/// The value argument `name` holds, or the usage failure for a missing one.
fn required<T>(name: &str, value: Result<Option<T>, Failure>) -> Result<T, Failure> {
    value?.ok_or_else(|| usage(&format!("{name} is required")))
}

/// The most Unicode scalar values string argument `name`, or each string
/// of array argument `name`, may hold: 256 for a base, 4096 for a path or
/// file, and 1024 for anything else.
fn max_length(name: &str) -> usize {
    match name {
        "against" => 256,
        "file" | "repo" | "path" | "focus" => 4096,
        _ => 1024,
    }
}

/// `text`, or the usage failure for one longer than [`max_length`]. The
/// message never holds the text.
fn bounded(name: &str, text: &str) -> Result<String, Failure> {
    let max = max_length(name);
    if text.chars().nth(max).is_some() {
        return Err(usage(&format!("{name} is longer than {max} characters")));
    }
    Ok(text.to_owned())
}

/// A tool's arguments, checked against the names it takes. Messages name
/// only the tool's own argument names, never a value or key the client
/// sent.
///
/// [`Args::read`] takes each reader's result and keeps the first failure
/// met, in the order the reads are made, so a tool reads its arguments as
/// one list and [`Args::checked`] then reports the first bad one, as if
/// each read had returned at once.
struct Args<'a> {
    map: &'a Map<String, Value>,
    first: RefCell<Option<Failure>>,
}

impl<'a> Args<'a> {
    fn new(map: &'a Map<String, Value>, allowed: &[&str]) -> Result<Self, Failure> {
        if map.keys().all(|key| allowed.contains(&key.as_str())) {
            Ok(Self {
                map,
                first: RefCell::new(None),
            })
        } else {
            Err(usage(&format!(
                "unknown argument; this tool takes {}",
                allowed.join(", ")
            )))
        }
    }

    fn string(&self, name: &str) -> Result<Option<String>, Failure> {
        match self.map.get(name) {
            None => Ok(None),
            Some(Value::String(text)) => bounded(name, text).map(Some),
            Some(_) => Err(usage(&format!("{name} must be a string"))),
        }
    }

    /// The value `value` holds, or its default when it holds a failure,
    /// which is kept when it is the first.
    fn read<T: Default>(&self, value: Result<T, Failure>) -> T {
        value.unwrap_or_else(|failure| {
            self.first.borrow_mut().get_or_insert(failure);
            T::default()
        })
    }

    /// The first failure [`Args::read`] kept, if any.
    fn checked(&self) -> Result<(), Failure> {
        self.first.take().map_or(Ok(()), Err)
    }

    fn required_string(&self, name: &str) -> Result<String, Failure> {
        required(name, self.string(name))
    }

    /// String `name`, read by `parse` as one of a fixed set, or the usage
    /// failure `message` for any other string.
    fn choice<T>(
        &self,
        name: &str,
        parse: impl Fn(&str) -> Option<T>,
        message: &str,
    ) -> Result<Option<T>, Failure> {
        self.string(name)?
            .map(|text| parse(&text).ok_or_else(|| usage(message)))
            .transpose()
    }

    /// `show_symbol`'s target: exactly one of `symbol` and `id`.
    fn target(&self) -> Result<Target, Failure> {
        match (self.string("symbol")?, self.string("id")?) {
            (Some(symbol), None) => Ok(Target::Name(symbol)),
            (None, Some(key)) => Ok(Target::Id(key)),
            _ => Err(usage("give exactly one of symbol and id")),
        }
    }

    /// `depth`, `min_confidence` and `include_tests`, read in that order,
    /// with the command line's defaults.
    fn impact_options(&self) -> Result<ImpactOptions, Failure> {
        let defaults = ImpactOptions::default();
        Ok(ImpactOptions {
            max_depth: self.depth()?.unwrap_or(defaults.max_depth),
            min_confidence: self.min_confidence()?.unwrap_or(defaults.min_confidence),
            include_tests: self.boolean("include_tests")?.unwrap_or(true),
        })
    }

    fn boolean(&self, name: &str) -> Result<Option<bool>, Failure> {
        match self.map.get(name) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(usage(&format!("{name} must be true or false"))),
        }
    }

    /// An array of strings, each at most [`max_length`], or empty when
    /// absent.
    fn strings(&self, name: &str) -> Result<Vec<String>, Failure> {
        let Some(value) = self.map.get(name) else {
            return Ok(Vec::new());
        };
        let items: Vec<&str> = value
            .as_array()
            .and_then(|items| items.iter().map(Value::as_str).collect())
            .ok_or_else(|| usage(&format!("{name} must be an array of strings")))?;
        items.into_iter().map(|item| bounded(name, item)).collect()
    }

    /// An integer from `min` to `max`. A number with no fraction, such as
    /// `2.0`, is an integer.
    fn integer(&self, name: &str, min: u64, max: u64) -> Result<Option<u64>, Failure> {
        let Some(value) = self.map.get(name) else {
            return Ok(None);
        };
        let number = value.as_u64().or_else(|| {
            value
                .as_f64()
                .filter(|n| n.fract() == 0.0 && *n >= 0.0)
                .map(|n| n as u64)
        });
        match number {
            Some(number) if (min..=max).contains(&number) => Ok(Some(number)),
            _ => Err(usage(&format!(
                "{name} must be an integer from {min} to {max}"
            ))),
        }
    }

    /// `focus`: an array of at most [`FOCUS_MAX`] strings, or empty when
    /// absent.
    fn focus(&self) -> Result<Vec<String>, Failure> {
        let focus = self.strings("focus")?;
        focus_cap(&focus)?;
        Ok(focus)
    }

    /// `min_lift`: a finite number from 0 to [`MIN_LIFT_MAX`], as the
    /// command line checks it.
    fn min_lift(&self) -> Result<Option<f64>, Failure> {
        let Some(value) = self.map.get("min_lift") else {
            return Ok(None);
        };
        match value.as_f64().filter(|number| min_lift_in_range(*number)) {
            Some(number) => Ok(Some(number)),
            None => Err(usage(&min_lift_usage())),
        }
    }

    /// `max_tokens`: an integer from [`MAX_TOKENS_MIN`] to
    /// [`MAX_TOKENS_MAX`].
    fn max_tokens(&self) -> Result<Option<u64>, Failure> {
        self.integer("max_tokens", MAX_TOKENS_MIN, MAX_TOKENS_MAX)
    }

    /// `depth`: an integer from 1 to [`ImpactOptions::MAX_DEPTH`]. A number
    /// with no fraction, such as `2.0`, is an integer.
    fn depth(&self) -> Result<Option<u8>, Failure> {
        let Some(value) = self.map.get("depth") else {
            return Ok(None);
        };
        let number = value.as_f64();
        (1..=ImpactOptions::MAX_DEPTH)
            .find(|depth| number == Some(f64::from(*depth)))
            .map(Some)
            .ok_or_else(|| {
                usage(&format!(
                    "depth must be an integer from 1 to {}",
                    ImpactOptions::MAX_DEPTH
                ))
            })
    }

    /// `min_confidence`: a number above 0 and at most 1, as the command
    /// line checks it.
    fn min_confidence(&self) -> Result<Option<f64>, Failure> {
        let Some(value) = self.map.get("min_confidence") else {
            return Ok(None);
        };
        let number = value.as_f64().filter(|number| {
            ImpactOptions {
                min_confidence: *number,
                ..ImpactOptions::default()
            }
            .in_range()
        });
        match number {
            Some(number) => Ok(Some(number)),
            None => Err(usage(
                "min_confidence must be a number above 0 and at most 1",
            )),
        }
    }
}
