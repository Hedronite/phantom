//! `find`: indexed symbols by exact name.

use std::path::PathBuf;

use phantom_fractal_lang::{Item, Visibility};
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::{Failure, canonical_repo, open_index};

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

/// One `find` result. `path` is relative to the indexed tree.
#[derive(Clone, Serialize)]
struct SymbolHit {
    name: String,
    kind: &'static str,
    visibility: String,
    path: String,
    line: u32,
}

impl SymbolHit {
    fn new(path: &str, item: Item) -> Self {
        let visibility = match item.visibility {
            Visibility::Private => "private".to_owned(),
            Visibility::Public => "pub".to_owned(),
            Visibility::Crate => "pub(crate)".to_owned(),
            Visibility::Restricted(text) => text,
        };
        Self {
            name: item.name,
            kind: item.kind.as_str(),
            visibility,
            path: path.to_owned(),
            line: item.start_line,
        }
    }
}

/// Indexed symbols under `request.repo` named exactly `request.query`,
/// ordered by path and then line, at most [`FIND_CAP`] of them, fitted
/// to `request.max_tokens`.
pub(crate) fn find(request: &FindRequest) -> Result<FindResult, Failure> {
    budget::check(request.max_tokens)?;
    budget::fit(matches(request)?, request.max_tokens)
}

/// The matches before any budget. The store's bound FTS match answers an
/// absent name without a walk. Otherwise each `.rs` file the walk keeps is read back from the store.
fn matches(request: &FindRequest) -> Result<FindResult, Failure> {
    let query = request.query.as_str();
    let store = open_index(&request.repo)?;
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
    let mut hits = Vec::new();
    for file in walk.files.iter().filter(|file| file.path.ends_with(".rs")) {
        for item in store.read_items(&file.path)? {
            if item.name == query {
                hits.push(SymbolHit::new(&file.path, item));
            }
        }
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    let truncated = hits.len() > FIND_CAP;
    hits.truncate(FIND_CAP);
    Ok(FindResult {
        hits,
        truncated,
        tokens: None,
    })
}
