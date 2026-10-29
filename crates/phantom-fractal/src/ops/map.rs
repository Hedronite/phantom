//! `map`: the budgeted repo map of an index (SPEC-w7 G3a), as
//! `phantom_fractal_analyze::render` packs it. Nothing is written.

use std::path::PathBuf;

use phantom_fractal_analyze::{Detail, MapError, MapRequest as Render, render};
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;

use super::Failure;
use super::lookup::current_store;
use super::rank::focus_files;

/// The smallest and largest budget `map` takes, in estimated tokens.
pub(crate) const BUDGET_MIN: u64 = 1;
pub(crate) const BUDGET_MAX: u64 = 100_000;

/// The detail level named `text`: `files`, `symbols`, or `full`.
pub(crate) fn detail(text: &str) -> Option<Detail> {
    match text {
        "files" => Some(Detail::Files),
        "symbols" => Some(Detail::Symbols),
        "full" => Some(Detail::Full),
        _ => None,
    }
}

/// What `map` is asked for.
pub(crate) struct MapRequest {
    /// [`BUDGET_MIN`] to [`BUDGET_MAX`] estimated tokens.
    pub(crate) budget: u64,
    /// Indexed file paths, relative to the repo, to put first.
    pub(crate) focus: Vec<String>,
    pub(crate) detail: Detail,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
}

/// `data` of a successful `map`, with its keys in this order.
#[derive(Serialize)]
pub(crate) struct MapResult {
    text: String,
    used_tokens: u64,
    budget: u64,
    files_shown: u64,
    files_omitted: u64,
    symbols_shown: u64,
    symbols_omitted: u64,
    rank_epoch: u64,
}

/// The map of `request.repo` within `request.budget`. A budget too small
/// for the map's header and footer is `Usage`, with a message that starts
/// `budget_too_small`.
pub(crate) fn map(request: &MapRequest) -> Result<MapResult, Failure> {
    if !(BUDGET_MIN..=BUDGET_MAX).contains(&request.budget) {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!("budget must be {BUDGET_MIN} to {BUDGET_MAX}"),
        ));
    }
    let store = current_store(&request.repo)?;
    let focus = focus_files(&store, &request.focus)?;
    let rendered = render(
        &store,
        &Render {
            budget: request.budget,
            focus_files: focus.ids,
            detail: request.detail,
        },
    )
    .map_err(|err| match err {
        MapError::Store(err) => err.into(),
        err @ MapError::BudgetTooSmall { .. } => {
            Failure::new(ErrorKind::Usage, format!("budget_too_small: {err}"))
        }
        err => Failure::new(ErrorKind::Usage, err.to_string()),
    })?;
    Ok(MapResult {
        text: rendered.text,
        used_tokens: rendered.used_tokens,
        budget: rendered.budget,
        files_shown: rendered.files_shown,
        files_omitted: rendered.files_omitted,
        symbols_shown: rendered.symbols_shown,
        symbols_omitted: rendered.symbols_omitted,
        rank_epoch: rendered.rank_epoch,
    })
}
