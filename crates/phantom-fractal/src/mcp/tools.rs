//! The tools: their definitions, their argument checks, and the
//! [`crate::ops`] call behind each one (SPEC-w6 G2b, SPEC-w7 G3b).
//!
//! A tool never does an operation's work itself. It turns its arguments
//! into the request the command line builds for the same flags, calls the
//! operation, and returns the envelope the command line prints.

use std::path::PathBuf;

use phantom_fractal_analyze::ImpactOptions;
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::ops::map::{BUDGET_MAX, BUDGET_MIN};
use crate::ops::rank::{DEFAULT_LIMIT, LIMIT_MAX, LIMIT_MIN};
use crate::ops::{
    self, CallsRequest, Failure, FindRequest, ImpactRequest, IndexRequest, MapRequest, RankKind,
    RankRequest, ShowRequest, Target,
};

/// One tool call's envelope line and whether it reports a failure.
pub(crate) struct Outcome {
    /// The envelope exactly as the command line prints it, without the
    /// newline.
    pub(crate) envelope: String,
    /// True exactly when the envelope's `ok` is false.
    pub(crate) is_error: bool,
}

const REPO: &str = "The indexed tree. Defaults to the server's working directory; a relative \
    path resolves against it.";

const SYMBOL: &str = "A bare name such as `parse`, or a qualified one such as `config::parse` or \
    `Config::load`.";

const FOCUS: &str = "Indexed files, each relative to the repo, such as `src/lib.rs`.";

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
                        "description": "The tree to index. A relative path resolves against the \
                            server's working directory.",
                    },
                }),
                &["path"],
            ),
            "annotations": { "readOnlyHint": false, "destructiveHint": false },
        }));
    }
    tools
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
) -> Option<Result<Outcome, ()>> {
    let outcome = match name {
        "find_symbols" => find_symbols(arguments),
        "show_symbol" => show_symbol(arguments),
        "callers" => calls(arguments, ops::callers),
        "callees" => calls(arguments, ops::callees),
        "impact" => impact(arguments),
        "top_ranked" => run(rank_request(arguments), ops::rank),
        "repo_map" => run(map_request(arguments), ops::map),
        "index_repo" if allow_index => index_repo(arguments),
        _ => return None,
    };
    Some(outcome)
}

fn find_symbols(arguments: &Map<String, Value>) -> Result<Outcome, ()> {
    run(find_request(arguments), ops::find)
}

fn find_request(arguments: &Map<String, Value>) -> Result<FindRequest, Failure> {
    let args = Args::new(arguments, &["query", "repo"])?;
    Ok(FindRequest {
        query: args.required_string("query")?,
        repo: args.repo()?,
    })
}

fn show_symbol(arguments: &Map<String, Value>) -> Result<Outcome, ()> {
    run(show_request(arguments), ops::show)
}

fn show_request(arguments: &Map<String, Value>) -> Result<ShowRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "id", "repo", "source"])?;
    let target = match (args.string("symbol")?, args.string("id")?) {
        (Some(symbol), None) => Target::Name(symbol),
        (None, Some(key)) => Target::Id(key),
        _ => return Err(usage("give exactly one of symbol and id")),
    };
    Ok(ShowRequest {
        target,
        repo: args.repo()?,
        source: args.boolean("source")?.unwrap_or(false),
    })
}

fn calls<T: Serialize>(
    arguments: &Map<String, Value>,
    op: fn(&CallsRequest) -> Result<T, Failure>,
) -> Result<Outcome, ()> {
    run(calls_request(arguments), op)
}

fn calls_request(arguments: &Map<String, Value>) -> Result<CallsRequest, Failure> {
    let args = Args::new(arguments, &["symbol", "repo"])?;
    Ok(CallsRequest {
        symbol: args.required_string("symbol")?,
        repo: args.repo()?,
    })
}

fn impact(arguments: &Map<String, Value>) -> Result<Outcome, ()> {
    run(impact_request(arguments), ops::impact)
}

fn impact_request(arguments: &Map<String, Value>) -> Result<ImpactRequest, Failure> {
    let args = Args::new(
        arguments,
        &[
            "symbol",
            "repo",
            "depth",
            "min_confidence",
            "include_tests",
            "summary",
        ],
    )?;
    let defaults = ImpactOptions::default();
    let options = ImpactOptions {
        max_depth: args.depth()?.unwrap_or(defaults.max_depth),
        min_confidence: args.min_confidence()?.unwrap_or(defaults.min_confidence),
        include_tests: args.boolean("include_tests")?.unwrap_or(true),
    };
    Ok(ImpactRequest {
        symbol: args.required_string("symbol")?,
        repo: args.repo()?,
        options,
        summary: args.boolean("summary")?.unwrap_or(false),
    })
}

fn rank_request(arguments: &Map<String, Value>) -> Result<RankRequest, Failure> {
    let args = Args::new(arguments, &["kind", "focus", "limit", "repo"])?;
    let kind = match args.string("kind")? {
        None => RankKind::Symbols,
        Some(text) => {
            RankKind::parse(&text).ok_or_else(|| usage("kind must be symbols or files"))?
        }
    };
    Ok(RankRequest {
        kind,
        focus: args.strings("focus")?,
        limit: args
            .integer("limit", LIMIT_MIN, LIMIT_MAX)?
            .unwrap_or(DEFAULT_LIMIT),
        repo: args.repo()?,
    })
}

fn map_request(arguments: &Map<String, Value>) -> Result<MapRequest, Failure> {
    let args = Args::new(arguments, &["budget", "focus", "detail", "repo"])?;
    let detail = match args.string("detail")? {
        None => Default::default(),
        Some(text) => {
            ops::map::detail(&text).ok_or_else(|| usage("detail must be files, symbols or full"))?
        }
    };
    Ok(MapRequest {
        budget: args
            .integer("budget", BUDGET_MIN, BUDGET_MAX)?
            .ok_or_else(|| usage("budget is required"))?,
        focus: args.strings("focus")?,
        detail,
        repo: args.repo()?,
    })
}

fn index_repo(arguments: &Map<String, Value>) -> Result<Outcome, ()> {
    run(index_request(arguments), ops::index)
}

fn index_request(arguments: &Map<String, Value>) -> Result<IndexRequest, Failure> {
    let args = Args::new(arguments, &["path"])?;
    Ok(IndexRequest {
        path: PathBuf::from(args.required_string("path")?),
    })
}

/// Calls `op` on a well-formed request, or returns the usage envelope of a
/// bad one.
fn run<R, T: Serialize>(
    request: Result<R, Failure>,
    op: impl FnOnce(&R) -> Result<T, Failure>,
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

    /// `repo`, or the working directory, as the command line defaults it.
    fn repo(&self) -> Result<PathBuf, Failure> {
        Ok(PathBuf::from(
            self.string("repo")?.unwrap_or_else(|| ".".to_owned()),
        ))
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
