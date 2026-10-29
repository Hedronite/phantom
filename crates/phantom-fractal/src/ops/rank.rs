//! `rank`: the best-ranked symbols or files of an index, optionally
//! personalised to focus files (SPEC-w7 G3a).
//!
//! The scores are the stored ones when their epoch equals `graph_epoch`
//! and there is no focus; otherwise they are computed in memory. Nothing
//! is written.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;

use phantom_fractal_analyze::{Ranks, personalised, rank as compute};
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{Store, StoreError};
use serde::Serialize;

use super::Failure;
use super::lookup::current_index;

/// The fewest and most hits one `rank` returns.
pub(crate) const LIMIT_MIN: u64 = 1;
pub(crate) const LIMIT_MAX: u64 = 100;
/// The hits `rank` returns when no limit is given.
pub(crate) const DEFAULT_LIMIT: u64 = 20;

/// Which nodes `rank` lists.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RankKind {
    #[default]
    Symbols,
    Files,
}

impl RankKind {
    /// The kind named `text`, as `data.kind` spells it.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "symbols" => Some(Self::Symbols),
            "files" => Some(Self::Files),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Symbols => "symbols",
            Self::Files => "files",
        }
    }
}

/// What `rank` is asked for.
pub(crate) struct RankRequest {
    pub(crate) kind: RankKind,
    /// Indexed file paths, relative to the repo, to personalise the rank
    /// to. Empty for the global rank.
    pub(crate) focus: Vec<String>,
    /// [`LIMIT_MIN`] to [`LIMIT_MAX`].
    pub(crate) limit: u64,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
}

/// `data` of a successful `rank`, with its keys in this order.
#[derive(Serialize)]
pub(crate) struct RankResult {
    kind: &'static str,
    /// The graph epoch of the scores.
    epoch: u64,
    /// The focus paths as stored, sorted, each once.
    focus: Vec<String>,
    hits: Hits,
    /// True when more nodes were ranked than `hits` holds.
    truncated: bool,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Hits {
    Symbols(Vec<SymbolHit>),
    Files(Vec<FileHit>),
}

/// One ranked symbol. `name` is the bare name and `qual_name` the name
/// `callers` prints for it.
#[derive(Serialize)]
struct SymbolHit {
    name: String,
    qual_name: String,
    kind: String,
    path: String,
    line: Option<i64>,
    score: f64,
}

/// One ranked file.
#[derive(Serialize)]
struct FileHit {
    path: String,
    score: f64,
}

/// The top `request.limit` symbols or files, best first. Ties break by
/// path, then line.
pub(crate) fn rank(request: &RankRequest) -> Result<RankResult, Failure> {
    if !(LIMIT_MIN..=LIMIT_MAX).contains(&request.limit) {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!("limit must be {LIMIT_MIN} to {LIMIT_MAX}"),
        ));
    }
    let (store, names) = current_index(&request.repo)?;
    let focus = focus_files(&store, &request.focus)?;
    let ranks = ranks(&store, &focus.ids)?;
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let (hits, total) = match request.kind {
        RankKind::Symbols => {
            let hits = top_symbols(&store, &ranks, limit, |id| names.of(id))?;
            (Hits::Symbols(hits), ranks.symbols.len())
        }
        RankKind::Files => {
            let hits = top_files(&store, &ranks, limit)?;
            (Hits::Files(hits), ranks.files.len())
        }
    };
    Ok(RankResult {
        kind: request.kind.as_str(),
        epoch: ranks.epoch,
        focus: focus.paths,
        hits,
        truncated: total > limit,
    })
}

/// Focus paths matched to indexed files.
pub(crate) struct Focus {
    /// The stored paths, sorted, each once.
    pub(crate) paths: Vec<String>,
    /// Their file ids, in the same order.
    pub(crate) ids: Vec<i64>,
}

/// The indexed files `paths` name, each relative to the repo. A leading
/// `./` is allowed. A path that names no indexed file is `NotFound`; the
/// message gives its position, never the path, which may be absolute.
pub(crate) fn focus_files(store: &Store, paths: &[String]) -> Result<Focus, Failure> {
    let files: BTreeMap<String, i64> = store
        .files()?
        .into_iter()
        .map(|file| (file.path, file.id))
        .collect();
    let mut found = BTreeMap::new();
    for (position, path) in paths.iter().enumerate() {
        let mut relative = path.as_str();
        while let Some(rest) = relative.strip_prefix("./") {
            relative = rest;
        }
        let Some((stored, id)) = files.get_key_value(relative) else {
            return Err(Failure::new(
                ErrorKind::NotFound,
                format!(
                    "focus path {} of {} is not an indexed file; give paths relative to the repo",
                    position + 1,
                    paths.len()
                ),
            ));
        };
        found.insert(stored.clone(), *id);
    }
    Ok(Focus {
        ids: found.values().copied().collect(),
        paths: found.into_keys().collect(),
    })
}

/// The ranks for `focus`: the stored ones when they are current and there
/// is no focus, else computed now.
pub(crate) fn ranks(store: &Store, focus: &[i64]) -> Result<Ranks, StoreError> {
    if !focus.is_empty() {
        return personalised(store, focus);
    }
    let epoch = store.graph_epoch()?;
    if epoch > 0 && store.rank_epoch()? == epoch {
        let mut symbols = Vec::new();
        let mut files = Vec::new();
        for (kind, id, score) in store.stored_rank()? {
            match kind.as_str() {
                "symbol" => symbols.push((id, score)),
                "file" => files.push((id, score)),
                _ => {}
            }
        }
        if !symbols.is_empty() || !files.is_empty() {
            return Ok(Ranks {
                symbols,
                files,
                epoch,
            });
        }
    }
    compute(store)
}

/// Higher scores first; NaN sorts as the lowest.
fn descending(a: f64, b: f64) -> Ordering {
    b.total_cmp(&a)
}

fn top_symbols(
    store: &Store,
    ranks: &Ranks,
    limit: usize,
    qualified: impl Fn(i64) -> Option<String>,
) -> Result<Vec<SymbolHit>, Failure> {
    let paths: BTreeMap<i64, String> = store
        .rank_input()?
        .into_iter()
        .map(|row| (row.id, row.path))
        .collect();
    let mut order: Vec<(f64, &str, i64)> = ranks
        .symbols
        .iter()
        .filter_map(|&(id, score)| Some((score, paths.get(&id)?.as_str(), id)))
        .collect();
    order.sort_by(|a, b| descending(a.0, b.0).then(a.1.cmp(b.1)).then(a.2.cmp(&b.2)));
    // Lines break ties inside a file, so the cut takes in every symbol
    // that ties the last one kept on score and path.
    let mut end = order.len().min(limit);
    while end > 0 && end < order.len() {
        let (last, next) = (order[end - 1], order[end]);
        if last.0.total_cmp(&next.0) != Ordering::Equal || last.1 != next.1 {
            break;
        }
        end += 1;
    }
    let mut hits = Vec::new();
    for &(score, _, id) in &order[..end] {
        let Some(detail) = store.symbol_detail(id)? else {
            continue;
        };
        hits.push((
            id,
            SymbolHit {
                qual_name: qualified(id).unwrap_or(detail.qual_name),
                name: detail.name,
                kind: detail.kind,
                path: detail.path,
                line: detail.start_line,
                score,
            },
        ));
    }
    hits.sort_by(|(a_id, a), (b_id, b)| {
        descending(a.score, b.score)
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(a_id.cmp(b_id))
    });
    hits.truncate(limit);
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}

fn top_files(store: &Store, ranks: &Ranks, limit: usize) -> Result<Vec<FileHit>, Failure> {
    let paths: BTreeMap<i64, String> = store
        .files()?
        .into_iter()
        .map(|file| (file.id, file.path))
        .collect();
    let mut hits: Vec<FileHit> = ranks
        .files
        .iter()
        .filter_map(|&(id, score)| {
            Some(FileHit {
                path: paths.get(&id)?.clone(),
                score,
            })
        })
        .collect();
    hits.sort_by(|a, b| descending(a.score, b.score).then(a.path.cmp(&b.path)));
    hits.truncate(limit);
    Ok(hits)
}
