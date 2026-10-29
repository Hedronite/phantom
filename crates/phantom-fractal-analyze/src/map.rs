//! The budgeted repo map (SPEC-w7 G2, canon §8).
//!
//! [`render`] packs files and symbols, best ranked first, into a text whose
//! estimated tokens never exceed the budget. Each line costs
//! [`estimate_tokens`] of it, `ceil(bytes / 3.5) + 1`, so `used_tokens` is
//! `estimate_tokens` of the whole text. Packing uses `floor(0.95 × budget)`
//! with room for the header and the footer reserved up front.
//!
//! The candidates form one fixed sequence of units in canon order: a
//! file's header with its top symbol, then each of its other symbols, then
//! the next file's header with its top symbol, and so on. In `full`
//! detail a symbol and its doc line are one unit; in `files` detail each
//! file line is one. The map is the longest prefix of that sequence that
//! fits, so a larger budget never drops a line a smaller one showed
//! (SPEC-w7 G2 step 6). Files go by rank, focus files first; symbols go
//! by rank within their file; ties break by path, then line.
//!
//! The ranks are the stored ones when their epoch equals `graph_epoch` and
//! there is no focus; otherwise they are computed in memory. Nothing is
//! written.

mod format;
mod store;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use phantom_fractal_store::{MapFile, MapRow, StoreError};

use crate::estimate_tokens;
use crate::rank::{RankSource, Ranks, personalised, rank};

/// What [`render`] reads beyond the rank inputs.
pub trait MapSource: RankSource {
    /// Every indexed file.
    fn map_files(&self) -> Result<Vec<MapFile>, StoreError>;
    /// Every non-test, non-module symbol with its map fields.
    fn map_rows(&self) -> Result<Vec<MapRow>, StoreError>;
    /// The epoch the stored rank was computed at, or 0 for none.
    fn rank_epoch(&self) -> Result<u64, StoreError>;
    /// The stored rank rows as `(node_kind, node_id, score)`, where the
    /// kind is `symbol` or `file`.
    fn stored_rank(&self) -> Result<Vec<(String, i64, f64)>, StoreError>;
}

/// How much of each file the map prints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Detail {
    /// File lines only.
    Files,
    /// File lines and symbol lines.
    #[default]
    Symbols,
    /// Symbol lines with a doc line under each documented symbol.
    Full,
}

/// One map to render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapRequest {
    /// The most estimated tokens the text may cost.
    pub budget: u64,
    /// File ids to put first, ranked by the rank personalised to them. An
    /// id with no file is ignored.
    pub focus_files: Vec<i64>,
    pub detail: Detail,
}

/// A rendered map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Map {
    /// The lines, each ending in `\n`. ASCII only.
    pub text: String,
    /// The estimated tokens of `text`, header and footer included.
    pub used_tokens: u64,
    pub budget: u64,
    pub files_shown: u64,
    pub files_omitted: u64,
    pub symbols_shown: u64,
    pub symbols_omitted: u64,
    /// The graph epoch of the ranks used.
    pub rank_epoch: u64,
}

/// Why [`render`] gave no map.
#[derive(Debug)]
#[non_exhaustive]
pub enum MapError {
    /// `floor(0.95 × budget)` cannot hold the header and the footer.
    BudgetTooSmall {
        budget: u64,
        /// The fewest estimated tokens the header and footer need.
        needed: u64,
    },
    Store(StoreError),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetTooSmall { budget, needed } => write!(
                f,
                "budget {budget} is too small: the header and footer need {needed} estimated \
                 tokens, and 95% of the budget is {}",
                packing_budget(*budget)
            ),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl Error for MapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BudgetTooSmall { .. } => None,
            Self::Store(err) => Some(err),
        }
    }
}

impl From<StoreError> for MapError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// The share of the budget packing may use, as a fraction `95 / 100`.
const MARGIN: (u64, u64) = (95, 100);

/// `floor(0.95 × budget)`.
pub fn packing_budget(budget: u64) -> u64 {
    budget.saturating_mul(MARGIN.0) / MARGIN.1
}

/// The map of `source` for `request`.
pub fn render(source: &impl MapSource, request: &MapRequest) -> Result<Map, MapError> {
    let mut files = source.map_files()?;
    files.sort_by_key(|file| file.id);
    let mut rows = source.map_rows()?;
    rows.sort_by_key(|row| row.id);
    let known: BTreeSet<i64> = files.iter().map(|file| file.id).collect();
    let focus: BTreeSet<i64> = request
        .focus_files
        .iter()
        .copied()
        .filter(|id| known.contains(id))
        .collect();
    let ranks = ranks_for(source, &focus)?;
    let symbol_rank: BTreeMap<i64, f64> = ranks.symbols.iter().copied().collect();
    let file_rank: BTreeMap<i64, f64> = ranks.files.iter().copied().collect();
    let score = |ranks: &BTreeMap<i64, f64>, id: i64| ranks.get(&id).copied().unwrap_or(0.0);

    files.sort_by(|a, b| {
        let focused = |id| !focus.contains(&id);
        focused(a.id)
            .cmp(&focused(b.id))
            .then(descending(score(&file_rank, a.id), score(&file_rank, b.id)))
            .then(a.path.cmp(&b.path))
            .then(a.id.cmp(&b.id))
    });
    let mut by_file: BTreeMap<i64, Vec<&MapRow>> = BTreeMap::new();
    for row in &rows {
        by_file.entry(row.file_id).or_default().push(row);
    }
    for symbols in by_file.values_mut() {
        symbols.sort_by(|a, b| {
            descending(score(&symbol_rank, a.id), score(&symbol_rank, b.id))
                .then(a.start_line.cmp(&b.start_line))
                .then(a.id.cmp(&b.id))
        });
    }

    let total_files = files.len() as u64;
    let total_symbols = by_file
        .iter()
        .filter(|(file, _)| known.contains(file))
        .map(|(_, symbols)| symbols.len() as u64)
        .sum::<u64>();
    let packing = packing_budget(request.budget);
    let reserved = estimate_tokens(&format::header(request.budget, request.budget, ranks.epoch))
        + estimate_tokens(&format::footer(total_files, total_symbols));
    if reserved > packing {
        return Err(MapError::BudgetTooSmall {
            budget: request.budget,
            needed: reserved,
        });
    }

    let mut room = packing - reserved;
    let mut body = Vec::new();
    let (mut files_shown, mut symbols_shown) = (0, 0);
    'files: for file in &files {
        let header = format::file_line(&file.path, score(&file_rank, file.id));
        let symbols = match request.detail {
            Detail::Files => &[][..],
            Detail::Symbols | Detail::Full => by_file.get(&file.id).map_or(&[][..], Vec::as_slice),
        };
        let mut lines = vec![header];
        for (position, row) in symbols.iter().enumerate() {
            let mut unit = std::mem::take(&mut lines);
            unit.push(format::symbol_line(
                &row.kind,
                &row.name,
                row.owner.as_deref(),
                row.signature.as_deref(),
                row.start_line,
            ));
            if request.detail == Detail::Full
                && let Some(doc) = row.doc_summary.as_deref().filter(|doc| !doc.is_empty())
            {
                unit.push(format::doc_line(doc));
            }
            if !take(&mut room, &mut body, unit) {
                break 'files;
            }
            if position == 0 {
                files_shown += 1;
            }
            symbols_shown += 1;
        }
        if symbols.is_empty() {
            if !take(&mut room, &mut body, lines) {
                break;
            }
            files_shown += 1;
        }
    }

    let files_omitted = total_files - files_shown;
    let symbols_omitted = total_symbols - symbols_shown;
    let footer = format::footer(files_omitted, symbols_omitted);
    let rest =
        body.iter().map(|line| estimate_tokens(line)).sum::<u64>() + estimate_tokens(&footer);
    // The header states the total it is part of, so its own cost depends
    // on the digits it prints; this rises to the one fixed point.
    let mut used = rest + estimate_tokens(&format::header(request.budget, 0, ranks.epoch));
    loop {
        let next = rest + estimate_tokens(&format::header(request.budget, used, ranks.epoch));
        if next == used {
            break;
        }
        used = next;
    }
    let mut text = format::header(request.budget, used, ranks.epoch);
    text.push('\n');
    for line in body.iter().chain([&footer]) {
        text.push_str(line);
        text.push('\n');
    }
    Ok(Map {
        text,
        used_tokens: used,
        budget: request.budget,
        files_shown,
        files_omitted,
        symbols_shown,
        symbols_omitted,
        rank_epoch: ranks.epoch,
    })
}

/// Appends `unit` to `body` when its lines fit in `room`, and takes their
/// cost from it. False, with nothing changed, when they do not.
fn take(room: &mut u64, body: &mut Vec<String>, unit: Vec<String>) -> bool {
    let cost: u64 = unit.iter().map(|line| estimate_tokens(line)).sum();
    if cost > *room {
        return false;
    }
    *room -= cost;
    body.extend(unit);
    true
}

/// Higher scores first; NaN sorts as the lowest.
fn descending(a: f64, b: f64) -> Ordering {
    b.total_cmp(&a)
}

/// The stored rank when it is current and there is no focus, else the
/// rank computed now.
fn ranks_for(source: &impl MapSource, focus: &BTreeSet<i64>) -> Result<Ranks, StoreError> {
    if !focus.is_empty() {
        let focus: Vec<i64> = focus.iter().copied().collect();
        return personalised(source, &focus);
    }
    let epoch = source.graph_epoch()?;
    if epoch > 0 && source.rank_epoch()? == epoch {
        let mut symbols = Vec::new();
        let mut files = Vec::new();
        for (kind, id, score) in source.stored_rank()? {
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
    rank(source)
}
