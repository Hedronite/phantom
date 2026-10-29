//! Finding the one symbol a query names, and naming stored symbols.
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
use phantom_fractal_store::{Store, SymbolMatch};
use serde::Serialize;

use super::{Failure, open_index};

/// Most candidates an `Ambiguous` failure lists.
const CANDIDATE_CAP: usize = 10;

/// One symbol, by qualified name. `path` is relative to the indexed tree.
#[derive(Serialize)]
pub(crate) struct SymbolOut {
    name: String,
    kind: String,
    path: String,
    line: Option<i64>,
    /// Set only on the candidates of a `show` lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_key: Option<String>,
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

    /// The resolver's symbol `id`, when the input holds it.
    pub(crate) fn symbol(&self, id: Id) -> Option<&Symbol> {
        self.symbols.get(&id)
    }

    /// `crate::a::Owner::name`. A symbol in an orphan file has no module,
    /// so its name starts at its inline scope.
    pub(crate) fn qualified(&self, symbol: &Symbol) -> String {
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
    find_one(store, names, query, false)
}

/// [`lookup`], with each `Ambiguous` candidate carrying its stable key.
pub(crate) fn lookup_keyed(
    store: &Store,
    names: &Names,
    query: &str,
) -> Result<SymbolMatch, Failure> {
    find_one(store, names, query, true)
}

fn find_one(
    store: &Store,
    names: &Names,
    query: &str,
    keyed: bool,
) -> Result<SymbolMatch, Failure> {
    let mut matches = store.symbols_matching(query)?;
    matches.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    match matches.len() {
        0 => Err(Failure::new(
            ErrorKind::NotFound,
            format!("no symbol named {query}"),
        )),
        1 => Ok(matches.remove(0)),
        total => {
            let mut candidates = Vec::new();
            for found in matches.into_iter().take(CANDIDATE_CAP) {
                let stable_key = if keyed {
                    store
                        .symbol_detail(found.id)?
                        .map(|detail| detail.stable_key)
                } else {
                    None
                };
                candidates.push(SymbolOut {
                    stable_key,
                    ..symbol_out(names, found)
                });
            }
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
        .of(found.id)
        .unwrap_or_else(|| found.qual_name.clone());
    SymbolOut {
        name,
        kind: found.kind,
        path: found.file,
        line: found.line,
        stable_key: None,
    }
}

/// The stored text of a symbol kind.
pub(crate) const fn kind_text(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Module => "module",
        SymbolKind::Method => "method",
    }
}
