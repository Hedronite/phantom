//! The tools: their definitions, their argument checks, and the
//! [`crate::ops`] call behind each one (SPEC-w6 G2b, SPEC-w7 G3b, SPEC-w8
//! G0b, G0c and G3b, SPEC-w9 G0c and G2b).
//!
//! A tool never does an operation's work itself. It turns its arguments
//! into the request the command line builds for the same flags, calls the
//! operation, and returns the envelope the command line prints. Its
//! `repo` is checked against the [`Roots`] last, after every other
//! argument.

use phantom_fractal_analyze::ImpactOptions;
use phantom_fractal_git::Window;
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::roots::Roots;
use crate::ops::budget::{MAX_TOKENS_MAX, MAX_TOKENS_MIN};
use crate::ops::cochange::{self as cochange, DEFAULT_MIN_LIFT, min_lift_in_range};
use crate::ops::map::{BUDGET_MAX, BUDGET_MIN};
use crate::ops::rank::{DEFAULT_LIMIT, FOCUS_MAX, LIMIT_MAX, LIMIT_MIN, focus_cap};
use crate::ops::{
    self, CallsRequest, CochangeRequest, DiffRequest, Failure, FindRequest, ImpactRequest,
    IndexRequest, MapRequest, RankKind, RankRequest, Reply, ShowRequest, StatusRequest, Target,
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

const SYMBOL: &str = "A bare name such as `parse`, or a qualified one such as `config::parse` or \
    `Config::load`.";

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
            "description": "Find indexed symbols whose name is exactly `query`. Returns at most \
                100, each with its kind, visibility, path and line.",
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
            "description": "Show one symbol: its signature, doc summary, span, test flag and \
                stable key, its members, up to 20 callers and callees, and its macro-argument \
                count. Give exactly one of `symbol` and `id`. Source lines come back only when \
                `source` is true.",
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
            "description": "List the resolved calls to one symbol, at most 100, with the \
                reason and confidence of each.",
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
            "description": "List the resolved calls one symbol makes, at most 100, with the \
                reason and confidence of each, and its count of unresolved calls.",
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
            "description": "Walk the calls into one symbol backwards to estimate what a change \
                to it reaches: hits by depth, tests apart, counts, the reasons the answer may be \
                incomplete, and a risk level.",
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
            "description": "List the best-ranked symbols or files of the index, by PageRank over \
                its calls, trait impls and imports, with each score. With `focus`, the rank is \
                personalised to those files.",
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
            "description": "A ranked map of the repo: files best first, each with its \
                best-ranked symbols' signatures and lines, packed so its estimated tokens never \
                exceed `budget`. With `focus`, those files come first.",
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
            "description": "List the files that tend to change with `file`, from the git \
                history `index` read on a git root: each partner with its lift, the decayed \
                weight of the commits they changed in together, the confidence that a change to \
                `file` comes with one to it, and the last time they changed together. Without a \
                scan, `scan` is null and there are no partners.",
            "inputSchema": schema(
                json!({
                    "file": {
                        "type": "string",
                        "description": "An indexed file, relative to the repo, such as \
                            `src/lib.rs`.",
                    },
                    "min_lift": {
                        "type": "number",
                        "minimum": 0,
                        "default": DEFAULT_MIN_LIFT,
                        "description": "The lowest lift listed.",
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
            "description": "Estimate what the uncommitted change reaches: the work tree is compared \
                with HEAD, or with `against`, by raw bytes, and the calls into every changed \
                symbol the index holds are walked backwards at once. Returns the base, the changed \
                symbols, the changed files with no changed symbol, whether a manifest changed, \
                and the impact: hits by depth, tests apart, counts, the reasons the answer may be \
                incomplete, and a risk level. On a git root only; staged and unstaged diffs are \
                not offered.",
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
            "name": "repo_status",
            "description": "Say whether the index is stale and why: how many files an index \
                pass would add, rewrite and remove, whether the workspace manifests changed, \
                HEAD now and at the last pass, the stored counts, versions and epochs, the \
                co-change scan, and whether a writer holds the index. Reads only; a stale index \
                is never re-indexed.",
            "inputSchema": schema(
                json!({
                    "repo": { "type": "string", "description": REPO },
                }),
                &[],
            ),
            "annotations": read,
        }),
    ];
    if allow_index {
        tools.push(json!({
            "name": "index_repo",
            "description": "Index the Rust files under `path` and resolve their calls. The \
                index is written to the data directory, never under `path`. A second run with \
                nothing changed writes nothing.",
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

fn schema(properties: Value, required: &[&str]) -> Value {
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
        "cochange" => run(cochange_request(arguments, roots), ops::cochange),
        "diff_impact" => run(diff_request(arguments, roots), ops::impact_diff),
        "repo_status" => run(status_request(arguments, roots), ops::status),
        "index_repo" if allow_index => run(index_request(arguments, roots), ops::index),
        _ => return None,
    };
    Some(outcome)
}

fn find_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<FindRequest, Failure> {
    let args = Args::new(arguments, &["query", "repo", "max_tokens"])?;
    let query = args.required_string("query")?;
    let repo = args.string("repo")?;
    let max_tokens = args.max_tokens()?;
    Ok(FindRequest {
        query,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
}

fn show_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<ShowRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "id", "repo", "source", "max_tokens"])?;
    let target = match (args.string("symbol")?, args.string("id")?) {
        (Some(symbol), None) => Target::Name(symbol),
        (None, Some(key)) => Target::Id(key),
        _ => return Err(usage("give exactly one of symbol and id")),
    };
    let repo = args.string("repo")?;
    let source = args.boolean("source")?.unwrap_or(false);
    let max_tokens = args.max_tokens()?;
    Ok(ShowRequest {
        target,
        repo: roots.indexed(repo.as_deref())?,
        source,
        max_tokens,
    })
}

fn calls_request(arguments: &Map<String, Value>, roots: &Roots) -> Result<CallsRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "repo", "max_tokens"])?;
    let symbol = args.required_string("symbol")?;
    let repo = args.string("repo")?;
    let max_tokens = args.max_tokens()?;
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
    let defaults = ImpactOptions::default();
    let options = ImpactOptions {
        max_depth: args.depth()?.unwrap_or(defaults.max_depth),
        min_confidence: args.min_confidence()?.unwrap_or(defaults.min_confidence),
        include_tests: args.boolean("include_tests")?.unwrap_or(true),
    };
    let symbol = args.required_string("symbol")?;
    let repo = args.string("repo")?;
    let summary = args.boolean("summary")?.unwrap_or(false);
    let max_tokens = args.max_tokens()?;
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
    let kind = match args.string("kind")? {
        None => RankKind::Symbols,
        Some(text) => {
            RankKind::parse(&text).ok_or_else(|| usage("kind must be symbols or files"))?
        }
    };
    let focus = args.focus()?;
    let limit = args
        .integer("limit", LIMIT_MIN, LIMIT_MAX)?
        .unwrap_or(DEFAULT_LIMIT);
    let repo = args.string("repo")?;
    let max_tokens = args.max_tokens()?;
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
    let detail = match args.string("detail")? {
        None => Default::default(),
        Some(text) => {
            ops::map::detail(&text).ok_or_else(|| usage("detail must be files, symbols or full"))?
        }
    };
    let budget = args
        .integer("budget", BUDGET_MIN, BUDGET_MAX)?
        .ok_or_else(|| usage("budget is required"))?;
    let focus = args.focus()?;
    let repo = args.string("repo")?;
    Ok(MapRequest {
        budget,
        focus,
        detail,
        repo: roots.indexed(repo.as_deref())?,
    })
}

fn cochange_request(
    arguments: &Map<String, Value>,
    roots: &Roots,
) -> Result<CochangeRequest, Failure> {
    let args = Args::new(
        arguments,
        &["file", "min_lift", "limit", "repo", "max_tokens"],
    )?;
    let file = args.required_string("file")?;
    let min_lift = args.min_lift()?.unwrap_or(DEFAULT_MIN_LIFT);
    let limit = args
        .integer("limit", cochange::LIMIT_MIN, cochange::LIMIT_MAX)?
        .unwrap_or(cochange::DEFAULT_LIMIT);
    let repo = args.string("repo")?;
    let max_tokens = args.max_tokens()?;
    Ok(CochangeRequest {
        file,
        min_lift,
        limit,
        repo: roots.indexed(repo.as_deref())?,
        max_tokens,
    })
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
    let defaults = ImpactOptions::default();
    let options = ImpactOptions {
        max_depth: args.depth()?.unwrap_or(defaults.max_depth),
        min_confidence: args.min_confidence()?.unwrap_or(defaults.min_confidence),
        include_tests: args.boolean("include_tests")?.unwrap_or(true),
    };
    let against = args.string("against")?;
    let summary = args.boolean("summary_only")?.unwrap_or(false);
    let max_tokens = args.max_tokens()?;
    let repo = args.string("repo")?;
    Ok(DiffRequest {
        against,
        repo: roots.indexed(repo.as_deref())?,
        options,
        summary,
        max_tokens,
    })
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
    let path = args.required_string("path")?;
    let days = args.integer("cochange_days", 1, u64::from(Window::MAX_DAYS))?;
    let commits = args.integer("cochange_commits", 1, u64::from(Window::MAX_COMMITS))?;
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

/// A tool's arguments, checked against the names it takes. Messages name
/// only the tool's own argument names, never a value or key the client
/// sent.
struct Args<'a> {
    map: &'a Map<String, Value>,
}

impl<'a> Args<'a> {
    fn new(map: &'a Map<String, Value>, allowed: &[&str]) -> Result<Self, Failure> {
        if map.keys().all(|key| allowed.contains(&key.as_str())) {
            Ok(Self { map })
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
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(usage(&format!("{name} must be a string"))),
        }
    }

    fn required_string(&self, name: &str) -> Result<String, Failure> {
        self.string(name)?
            .ok_or_else(|| usage(&format!("{name} is required")))
    }

    fn boolean(&self, name: &str) -> Result<Option<bool>, Failure> {
        match self.map.get(name) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(usage(&format!("{name} must be true or false"))),
        }
    }

    /// An array of strings, or empty when absent.
    fn strings(&self, name: &str) -> Result<Vec<String>, Failure> {
        let Some(value) = self.map.get(name) else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .ok_or_else(|| usage(&format!("{name} must be an array of strings")))
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

    /// `min_lift`: a finite number, at least 0, as the command line checks
    /// it.
    fn min_lift(&self) -> Result<Option<f64>, Failure> {
        let Some(value) = self.map.get("min_lift") else {
            return Ok(None);
        };
        match value.as_f64().filter(|number| min_lift_in_range(*number)) {
            Some(number) => Ok(Some(number)),
            None => Err(usage("min_lift must be a number at least 0")),
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
