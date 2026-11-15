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

use super::{Failure, budget, open_index};

/// Most candidates an `Ambiguous` failure lists.
const CANDIDATE_CAP: usize = 10;

/// One symbol, by qualified name. `path` is relative to the indexed tree.
#[derive(Clone, Serialize)]
pub(crate) struct SymbolOut {
    name: String,
    kind: String,
    path: String,
    line: Option<i64>,
    /// Set only on the candidates of a `show` lookup.
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_key: Option<String>,
    /// The symbol named by file, as `src/a.rs::Type::name`. Set only on
    /// the candidates of an `Ambiguous` failure (SPEC-w15 G1b), so a
    /// caller can pass it back to pick one.
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
}

/// `data` of an `Ambiguous` failure: the first [`CANDIDATE_CAP`] matches
/// and how many there were.
#[derive(Serialize)]
pub(crate) struct Candidates {
    candidates: Vec<SymbolOut>,
    total: usize,
}

/// `data` of a lookup's `NotFound` failure: up to [`CANDIDATE_CAP`]
/// symbols whose bare name is the query's last segment, each in the shape
/// of an `Ambiguous` candidate. The list is empty when there are none.
#[derive(Serialize)]
pub(crate) struct DidYouMean {
    did_you_mean: Vec<SymbolOut>,
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

    /// The names of every symbol `store` holds.
    pub(crate) fn of_store(store: &Store) -> Result<Self, Failure> {
        Ok(Self::new(store.resolve_input()?))
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

    /// `src/tui/app.rs::App::start_watcher`: the declaring file, then the
    /// inline scope, owner and name within it.
    pub(crate) fn display(symbol: &Symbol) -> String {
        let mut parts: Vec<&str> = vec![&symbol.file];
        if !symbol.scope.is_empty() {
            parts.push(&symbol.scope);
        }
        if let Some(owner) = &symbol.owner {
            parts.push(owner);
        }
        parts.push(&symbol.name);
        parts.join("::")
    }

    /// [`Names::display`] of symbol `id`, or `file::name` when the
    /// resolver input does not hold it.
    pub(crate) fn display_of(&self, id: Id, file: &str, name: &str) -> String {
        self.symbols
            .get(&id)
            .map_or_else(|| format!("{file}::{name}"), Self::display)
    }
}

/// The store for the tree at `repo` and its symbol names. It is `NoIndex`
/// in every case [`current_store`] refuses.
pub(crate) fn current_index(repo: &Path) -> Result<(Store, Names), Failure> {
    let store = current_store(repo)?;
    let names = Names::new(store.resolve_input()?);
    Ok((store, names))
}

/// [`current_index`], once `max_tokens` is checked first.
pub(crate) fn checked_index(
    max_tokens: Option<u64>,
    repo: &Path,
) -> Result<(Store, Names), Failure> {
    budget::check(max_tokens)?;
    current_index(repo)
}

/// The store for the tree at `repo`. It is `NoIndex` unless the index was
/// written by this extractor version, as well as in every case
/// [`open_index`] refuses.
pub(crate) fn current_store(repo: &Path) -> Result<Store, Failure> {
    let store = open_index(repo)?;
    if store.extractor_version()? != Some(EXTRACTOR_VERSION) {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "the index has no call edges; run index again",
        ));
    }
    Ok(store)
}

/// The one stored symbol `query` names. None is `NotFound`, with up to
/// [`CANDIDATE_CAP`] `did_you_mean` suggestions; more than one is
/// `Ambiguous`, with up to [`CANDIDATE_CAP`] candidates.
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
        0 => {
            let did_you_mean = listed(store, names, suggestions(store, query)?, keyed, false)?;
            Err(Failure::not_found(
                format!("no symbol named {query}"),
                DidYouMean { did_you_mean },
            ))
        }
        1 => Ok(matches.remove(0)),
        _ => Err(ambiguous(store, names, query, matches, keyed)?),
    }
}

/// The `Ambiguous` failure for the `matches` of `query`: the first
/// [`CANDIDATE_CAP`] as candidates, each named with its path and line in
/// the message, and how many there were.
fn ambiguous(
    store: &Store,
    names: &Names,
    query: &str,
    matches: Vec<SymbolMatch>,
    keyed: bool,
) -> Result<Failure, Failure> {
    let total = matches.len();
    let candidates = listed(store, names, matches, keyed, true)?;
    let spelled: Vec<String> = candidates
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
    let message = format!(
        "{total} symbols match {query}: {}{tail}",
        spelled.join(", ")
    );
    Ok(Failure::ambiguous(
        message,
        Candidates { candidates, total },
    ))
}

/// The first [`CANDIDATE_CAP`] of `found`, named, each with its stable
/// key when `keyed` and its `display` when `shown`.
fn listed(
    store: &Store,
    names: &Names,
    found: Vec<SymbolMatch>,
    keyed: bool,
    shown: bool,
) -> Result<Vec<SymbolOut>, Failure> {
    let mut out = Vec::new();
    for found in found.into_iter().take(CANDIDATE_CAP) {
        let stable_key = if keyed {
            store
                .symbol_detail(found.id)?
                .map(|detail| detail.stable_key)
        } else {
            None
        };
        let display = shown.then(|| names.display_of(found.id, &found.file, &found.qual_name));
        out.push(SymbolOut {
            stable_key,
            display,
            ..symbol_out(names, found)
        });
    }
    Ok(out)
}

/// The store's suggestions for a missed `query`: symbols whose bare name
/// is its last segment, by file and then line.
fn suggestions(store: &Store, query: &str) -> Result<Vec<SymbolMatch>, Failure> {
    let mut found = store.did_you_mean(query)?;
    found.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Ok(found)
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
        display: None,
    }
}

/// `reexported` and `confirm_with_grep` (SPEC-w15 G1b): set, both true,
/// on a `callers` or `impact` reply whose symbol the stored rows record
/// as re-exported by a `pub use`, since a caller reached through that
/// path may be missing from the answer. Absent otherwise.
#[derive(Clone, Copy, Serialize)]
pub(crate) struct Reexport {
    reexported: bool,
    confirm_with_grep: bool,
}

impl Reexport {
    /// The keys for stored symbol `id`, when the store flags it.
    pub(crate) fn of(store: &Store, id: i64) -> Result<Option<Self>, Failure> {
        Ok(store.is_reexported(id)?.then_some(Self {
            reexported: true,
            confirm_with_grep: true,
        }))
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
