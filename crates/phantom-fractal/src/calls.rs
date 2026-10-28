//! `callers` and `callees`: the stored call edges of one symbol.
//!
//! A query is a bare name such as `helper`, an in-file name such as
//! `Counter::new` or `tests::check`, or a module-qualified name such as
//! `util::helper` or `crate::util::helper`. The store's `symbols_matching`
//! decides which symbols it names.

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
pub(crate) struct SymbolOut {
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
pub(crate) struct Names {
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

    /// The qualified name of symbol `id`, when the resolver input holds it.
    pub(crate) fn of(&self, id: Id) -> Option<String> {
        self.symbols.get(&id).map(|symbol| self.qualified(symbol))
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
    let (store, names) = current_index(repo)?;
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

/// The store for the tree at `repo` and its symbol names. It is `NoIndex`
/// unless the index was written by this extractor version, as well as in
/// every case [`open_index`] refuses.
pub(crate) fn current_index(repo: &Path) -> Result<(Store, Names), Failure> {
    let store = open_index(repo)?;
    if store.extractor_version()? != Some(EXTRACTOR_VERSION) {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "the index has no call edges; run index again",
        ));
    }
    let names = Names::new(store.resolve_input()?);
    Ok((store, names))
}

/// The one stored symbol `query` names. None is `NotFound`; more than one
/// is `Ambiguous`, with up to [`CANDIDATE_CAP`] candidates.
pub(crate) fn lookup(store: &Store, names: &Names, query: &str) -> Result<SymbolMatch, Failure> {
    let mut matches = store.symbols_matching(query)?;
    matches.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    match matches.len() {
        0 => Err(Failure::new(
            ErrorKind::NotFound,
            format!("no symbol named {query}"),
        )),
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

pub(crate) fn symbol_out(names: &Names, found: SymbolMatch) -> SymbolOut {
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
