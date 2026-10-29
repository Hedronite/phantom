//! `callers` and `callees`: the stored call edges of one symbol.

use std::path::PathBuf;

use phantom_fractal_store::{CallHit, Store};
use serde::Serialize;

use super::Failure;
use super::lookup::{Names, SymbolOut, current_index, kind_text, lookup, symbol_out};

/// Most hits one `callers` or `callees` returns.
const CALL_CAP: usize = 100;

/// What `callers` and `callees` are asked for.
pub(crate) struct CallsRequest {
    /// A bare, in-file, or module-qualified name.
    pub(crate) symbol: String,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
}

/// Which side of the queried symbol's edges to return.
#[derive(Clone, Copy)]
pub(crate) enum Direction {
    /// Symbols whose calls resolve to the queried symbol.
    Callers,
    /// Symbols the queried symbol's calls resolve to.
    Callees,
}

/// `data` of a successful `callers` or `callees`.
#[derive(Serialize)]
pub(crate) struct CallResult {
    symbol: SymbolOut,
    hits: Vec<CallOut>,
    truncated: bool,
    /// Call, method-call, and macro references in the symbol with no edge.
    /// `callees` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    unresolved_calls: Option<i64>,
}

/// One edge. `name` and `kind` describe the other symbol, and
/// `symbol_path` is the file it is declared in. `path` and `line` are the
/// call site: the caller's file for `callers`, the queried symbol's file
/// for `callees`.
#[derive(Serialize)]
pub(crate) struct CallOut {
    name: String,
    kind: &'static str,
    path: String,
    line: Option<i64>,
    symbol_path: String,
    confidence: f64,
    reason: String,
}

/// The symbols whose calls resolve to the one `request.symbol` names,
/// sorted by path and then line, at most [`CALL_CAP`] of them.
pub(crate) fn callers(request: &CallsRequest) -> Result<CallResult, Failure> {
    calls(Direction::Callers, request)
}

/// The symbols the calls of the one `request.symbol` names resolve to,
/// sorted by path and then line, at most [`CALL_CAP`] of them.
pub(crate) fn callees(request: &CallsRequest) -> Result<CallResult, Failure> {
    calls(Direction::Callees, request)
}

fn calls(direction: Direction, request: &CallsRequest) -> Result<CallResult, Failure> {
    let (store, names) = current_index(&request.repo)?;
    let symbol = lookup(&store, &names, &request.symbol)?;
    let mut hits = edges(&store, &names, direction, symbol.id, &symbol.file)?;
    let unresolved_calls = match direction {
        Direction::Callers => None,
        Direction::Callees => Some(store.unresolved_calls(symbol.id)?),
    };
    let truncated = hits.len() > CALL_CAP;
    hits.truncate(CALL_CAP);
    Ok(CallResult {
        symbol: symbol_out(&names, symbol),
        hits,
        truncated,
        unresolved_calls,
    })
}

/// Every stored edge on one side of symbol `id`, declared in `file`, with
/// the other symbol named, sorted by path and then line.
pub(crate) fn edges(
    store: &Store,
    names: &Names,
    direction: Direction,
    id: i64,
    file: &str,
) -> Result<Vec<CallOut>, Failure> {
    // A call ref sits in the file of the symbol it is inside, so a callee's
    // call site is in the queried symbol's file. The store gives the
    // callee's own file there instead.
    let stored = match direction {
        Direction::Callers => store.callers(id)?,
        Direction::Callees => store
            .callees(id)?
            .into_iter()
            .map(|hit| CallHit {
                file: file.to_owned(),
                ..hit
            })
            .collect(),
    };
    let mut hits: Vec<CallOut> = stored
        .into_iter()
        .filter_map(|hit| call_out(names, hit))
        .collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(hits)
}

/// A stored edge with the other symbol named, its call site in `hit.file`.
/// An edge to a symbol the resolver input does not hold is dropped.
fn call_out(names: &Names, hit: CallHit) -> Option<CallOut> {
    let symbol = names.symbol(hit.symbol)?;
    Some(CallOut {
        name: names.qualified(symbol),
        kind: kind_text(symbol.kind),
        path: hit.file,
        line: hit.line,
        symbol_path: symbol.file.clone(),
        confidence: hit.confidence,
        reason: hit.reason,
    })
}
