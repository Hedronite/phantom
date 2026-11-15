//! `find`: indexed symbols by exact name, methods included (SPEC-w14
//! G1b).

use std::collections::BTreeSet;
use std::path::PathBuf;

use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{FindRow, Store, StoreError};
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::lookup::Names;
use super::{Failure, Reply, answer, canonical_repo, open_index};

/// Most hits one `find` returns.
const FIND_CAP: usize = 100;

/// What `find` is asked for.
pub(crate) struct FindRequest {
    /// The exact name to look for.
    pub(crate) query: String,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `find`. `truncated` is true when more than
/// [`FIND_CAP`] symbols matched and only the first are in `hits`.
#[derive(Clone, Serialize)]
pub(crate) struct FindResult {
    hits: Vec<SymbolHit>,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

impl Shed for FindResult {
    fn sheddable(&self) -> usize {
        self.hits.len()
    }

    fn shed(&mut self, mut count: usize) {
        drop_last(&mut self.hits, &mut count);
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// One `find` result. `path` is relative to the indexed tree. An item's
/// `name` is its bare name; a method's is qualified, as
/// `crate::meta::Meta::page`, since its bare name alone does not say
/// whose it is.
#[derive(Clone, Serialize)]
struct SymbolHit {
    name: String,
    kind: String,
    visibility: String,
    path: String,
    line: u32,
}

impl SymbolHit {
    /// The hit for one stored row. `names` qualifies a method, and is
    /// read only when a method matched.
    fn new(row: FindRow, names: Option<&Names>) -> Result<Self, Failure> {
        let line = row
            .line
            .and_then(|line| u32::try_from(line).ok())
            .ok_or(StoreError::UnreadableStamp)?;
        let name = match names {
            Some(names) if row.kind == "method" => names.of(row.id).unwrap_or(row.name),
            _ => row.name,
        };
        Ok(Self {
            name,
            visibility: visibility_text(&row.visibility),
            kind: row.kind,
            path: row.file,
            line,
        })
    }
}

/// The stored visibility as `find` has always printed it: `pub`,
/// `pub(crate)`, `private`, or the restriction as written.
fn visibility_text(stored: &str) -> String {
    match stored {
        "public" => "pub".to_owned(),
        "crate" => "pub(crate)".to_owned(),
        other => other
            .strip_prefix("restricted:")
            .unwrap_or(other)
            .to_owned(),
    }
}

/// Indexed symbols under `request.repo` named exactly `request.query`,
/// ordered by path and then line, at most [`FIND_CAP`] of them, fitted
/// to `request.max_tokens`.
pub(crate) fn find(request: &FindRequest) -> Result<Reply<FindResult>, Failure> {
    budget::check(request.max_tokens)?;
    let store = open_index(&request.repo)?;
    let result = matches(request, &store)?;
    answer(result, &store, &request.repo, request.max_tokens)
}

/// The matches before any budget. The store's bound FTS match answers an
/// absent name without a walk. Otherwise the store's rows so named are
/// kept when the walk still keeps their `.rs` file.
fn matches(request: &FindRequest, store: &Store) -> Result<FindResult, Failure> {
    let query = request.query.as_str();
    if query.is_empty() || store.match_count(query)? == 0 {
        return Ok(FindResult {
            hits: Vec::new(),
            truncated: false,
            tokens: None,
        });
    }
    let root = canonical_repo(&request.repo)?;
    let walk = phantom_fractal_extract::walk(&root)
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("walk failed: {err}")))?;
    let kept: BTreeSet<&str> = walk
        .files
        .iter()
        .map(|file| file.path.as_str())
        .filter(|path| path.ends_with(".rs"))
        .collect();
    let rows: Vec<FindRow> = store
        .symbols_named(query)?
        .into_iter()
        .filter(|row| kept.contains(row.file.as_str()))
        .collect();
    let names = if rows.iter().any(|row| row.kind == "method") {
        Some(Names::of_store(store)?)
    } else {
        None
    };
    let mut hits = rows
        .into_iter()
        .map(|row| SymbolHit::new(row, names.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = hits.len() > FIND_CAP;
    hits.truncate(FIND_CAP);
    Ok(FindResult {
        hits,
        truncated,
        tokens: None,
    })
}
