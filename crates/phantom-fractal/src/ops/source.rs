//! A symbol's source lines for `show --source`.
//!
//! The file is opened only under the canonical indexed root: a path that
//! is missing, or canonicalizes outside the root (a symlink out, say), is
//! `unreadable`. Text is returned only when the file still has the blake3
//! the index stored and the item fits the caps.

use std::fs;
use std::path::Path;

use phantom_fractal_store::{Store, SymbolDetail};
use serde::Serialize;

use super::Failure;

/// Most lines of source one `show` returns.
const MAX_LINES: i64 = 400;

/// Most bytes of source one `show` returns: 32 KiB.
const MAX_BYTES: usize = 32 * 1024;

/// `source` of `show`. `text` is the item's lines, exactly as in the file,
/// and is present only when `status` is `ok`.
#[derive(Clone, Serialize)]
pub(crate) struct SourceOut {
    status: &'static str,
    start_line: Option<i64>,
    end_line: Option<i64>,
    text: Option<String>,
}

/// The source of `detail` under the canonical indexed `root`.
pub(crate) fn read_source(
    store: &Store,
    root: &Path,
    detail: &SymbolDetail,
) -> Result<SourceOut, Failure> {
    let (start, end) = (detail.start_line, detail.end_line);
    let out = |status, text| SourceOut {
        status,
        start_line: start,
        end_line: end,
        text,
    };
    let Some(text) = read_under(root, &detail.path) else {
        return Ok(out("unreadable", None));
    };
    let stored = store.stored_blake3(&detail.path)?;
    if stored.as_deref() != Some(blake3::hash(text.as_bytes()).as_bytes().as_slice()) {
        return Ok(out("stale", None));
    }
    let (Some(start), Some(end)) = (start, end) else {
        return Ok(out("unreadable", None));
    };
    if end - start + 1 > MAX_LINES {
        return Ok(out("too_large", None));
    }
    let Some(lines) = line_range(&text, start, end) else {
        return Ok(out("unreadable", None));
    };
    if lines.len() > MAX_BYTES {
        return Ok(out("too_large", None));
    }
    Ok(out("ok", Some(lines.to_owned())))
}

/// The UTF-8 text of the regular file at `path` under `root`, or nothing
/// when it is missing, canonicalizes outside `root`, is not a regular
/// file, or is not UTF-8.
fn read_under(root: &Path, path: &str) -> Option<String> {
    let file = root.join(path).canonicalize().ok()?;
    if !file.starts_with(root) || !fs::metadata(&file).ok()?.is_file() {
        return None;
    }
    String::from_utf8(fs::read(&file).ok()?).ok()
}

/// Lines `start` to `end` of `text`, 1-based and inclusive, each with its
/// line ending, or nothing when the range is empty or past the end.
fn line_range(text: &str, start: i64, end: i64) -> Option<&str> {
    if start < 1 || end < start {
        return None;
    }
    let mut offset = 0;
    let mut from = None;
    for (number, line) in (1..).zip(text.split_inclusive('\n')) {
        if number == start {
            from = Some(offset);
        }
        offset += line.len();
        if number == end {
            return from.map(|from| &text[from..offset]);
        }
    }
    None
}
