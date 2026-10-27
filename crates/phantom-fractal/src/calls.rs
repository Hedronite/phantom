//! `callers` and `callees`: the stored call edges of one symbol.
//!
//! A query is a bare name such as `helper` or a module-qualified name such
//! as `util::helper`, `crate::util::helper`, or `Counter::new`. Its
//! segments must equal the last segments of a symbol's qualified name.

use std::collections::HashMap;
use std::path::Path;

use phantom_fractal_extract::EXTRACTOR_VERSION;
use phantom_fractal_lang::{Id, ResolveInput, Symbol, SymbolKind};
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{CallHit, Store, SymbolMatch};
use serde::Serialize;

use crate::{Failure, open_index};

/// Most hits one `callers` or `callees` returns.
const CALL_CAP: usize = 100;

/// Most candidates an `Ambiguous` failure lists.
const CANDIDATE_CAP: usize = 10;

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

/// One symbol, by qualified name. `path` is relative to the indexed tree.
#[derive(Serialize)]
struct SymbolOut {
    name: String,
    kind: String,
    path: String,
    line: Option<i64>,
}

/// One edge. `name` and `kind` describe the other symbol, and
/// `symbol_path` is the file it is declared in. `path` and `line` are the
/// call site: the caller's file for `callers`, the queried symbol's file
/// for `callees`.
#[derive(Serialize)]
struct CallOut {
    name: String,
    kind: &'static str,
    path: String,
    line: Option<i64>,
    symbol_path: String,
    confidence: f64,
    reason: String,
}

/// `data` of an `Ambiguous` failure: the first [`CANDIDATE_CAP`] matches
/// and how many there were.
#[derive(Serialize)]
pub(crate) struct Candidates {
    candidates: Vec<SymbolOut>,
    total: usize,
}

/// Qualified names for every stored symbol, built from the resolver input.
struct Names {
    symbols: HashMap<Id, Symbol>,
    modules: HashMap<Id, String>,
}

impl Names {
    fn new(input: ResolveInput) -> Self {
        Self {
            symbols: input
                .symbols
                .into_iter()
                .map(|symbol| (symbol.id, symbol))
                .collect(),
            modules: input
                .modules
                .into_iter()
                .map(|module| (module.id, module.path))
                .collect(),
        }
    }

    /// `crate::a::Owner::name`. A symbol in an orphan file has no module,
    /// so its name starts at its inline scope.
    fn qualified(&self, symbol: &Symbol) -> String {
        let mut parts: Vec<&str> = Vec::new();
        match symbol.module.and_then(|id| self.modules.get(&id)) {
            Some(path) => parts.push(path),
            None if !symbol.scope.is_empty() => parts.push(&symbol.scope),
            None => {}
        }
        if let Some(owner) = &symbol.owner {
            parts.push(owner);
        }
        parts.push(&symbol.name);
        parts.join("::")
    }
}

/// The edges of the one symbol `query` names, sorted by path and then
/// line, at most [`CALL_CAP`] of them.
pub(crate) fn calls(direction: Direction, query: &str, repo: &Path) -> Result<CallResult, Failure> {
    let store = open_index(repo)?;
    if store.extractor_version()? != Some(EXTRACTOR_VERSION) {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "the index has no call edges; run index again",
        ));
    }
    let names = Names::new(store.resolve_input()?);
    let symbol = lookup(&store, &names, query)?;
    // A call ref sits in the file of the symbol it is inside, so a callee's
    // call site is in the queried symbol's file. The store gives the
    // callee's own file there instead.
    let (stored, unresolved_calls) = match direction {
        Direction::Callers => (store.callers(symbol.id)?, None),
        Direction::Callees => (
            store
                .callees(symbol.id)?
                .into_iter()
                .map(|hit| CallHit {
                    file: symbol.file.clone(),
                    ..hit
                })
                .collect(),
            Some(store.unresolved_calls(symbol.id)?),
        ),
    };
    let mut hits: Vec<CallOut> = stored
        .into_iter()
        .filter_map(|hit| call_out(&names, hit))
        .collect();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = hits.len() > CALL_CAP;
    hits.truncate(CALL_CAP);
    Ok(CallResult {
        symbol: symbol_out(&names, symbol),
        hits,
        truncated,
        unresolved_calls,
    })
}

/// The one stored symbol `query` names. None is `NotFound`; more than one
/// is `Ambiguous`, with up to [`CANDIDATE_CAP`] candidates.
fn lookup(store: &Store, names: &Names, query: &str) -> Result<SymbolMatch, Failure> {
    let segments: Vec<&str> = query.split("::").collect();
    let not_found = || Failure::new(ErrorKind::NotFound, format!("no symbol named {query}"));
    let Some(&name) = segments.last() else {
        return Err(not_found());
    };
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(not_found());
    }
    let mut matches: Vec<SymbolMatch> = store
        .symbols_matching(name)?
        .into_iter()
        .filter(|found| {
            names.symbols.get(&found.id).is_some_and(|symbol| {
                symbol.name == name && ends_with(&names.qualified(symbol), &segments)
            })
        })
        .collect();
    matches.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    match matches.len() {
        0 => Err(not_found()),
        1 => Ok(matches.remove(0)),
        total => {
            let candidates: Vec<SymbolOut> = matches
                .into_iter()
                .take(CANDIDATE_CAP)
                .map(|found| symbol_out(names, found))
                .collect();
            let listed: Vec<String> = candidates
                .iter()
                .map(|candidate| match candidate.line {
                    Some(line) => format!("{} ({}:{line})", candidate.name, candidate.path),
                    None => format!("{} ({})", candidate.name, candidate.path),
                })
                .collect();
            let more = total - candidates.len();
            let tail = if more > 0 {
                format!(", and {more} more")
            } else {
                String::new()
            };
            let message = format!("{total} symbols match {query}: {}{tail}", listed.join(", "));
            Err(Failure::ambiguous(
                message,
                Candidates { candidates, total },
            ))
        }
    }
}

/// True when the last segments of `qualified` are `segments`.
fn ends_with(qualified: &str, segments: &[&str]) -> bool {
    let parts: Vec<&str> = qualified.split("::").collect();
    parts.len() >= segments.len() && parts[parts.len() - segments.len()..] == *segments
}

fn symbol_out(names: &Names, found: SymbolMatch) -> SymbolOut {
    let name = names
        .symbols
        .get(&found.id)
        .map_or_else(|| found.qual_name.clone(), |symbol| names.qualified(symbol));
    SymbolOut {
        name,
        kind: found.kind,
        path: found.file,
        line: found.line,
    }
}

/// A stored edge with the other symbol named, its call site in `hit.file`.
/// An edge to a symbol the resolver input does not hold is dropped.
fn call_out(names: &Names, hit: CallHit) -> Option<CallOut> {
    let symbol = names.symbols.get(&hit.symbol)?;
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

/// The stored text of a symbol kind.
const fn kind_text(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Module => "module",
        SymbolKind::Method => "method",
    }
}
