//! `watch`: keep one index fresh while its tree is edited (SPEC-w9 G1,
//! D5).
//!
//! It polls. Every interval it runs the freshness probe, the same one
//! behind every reply's `meta`. When the probe says stale, it waits the
//! debounce and probes again, until two probes in a row agree, and then
//! runs one index pass, the same pass `index` runs. Each pass is a batch,
//! and each batch prints one envelope line. Nothing prints while the
//! index is fresh.
//!
//! The database's writer lock is taken at start and held until the
//! process ends, so no `index` or second `watch` writes beside it. There
//! is no signal handler: each pass commits in its own transactions, so a
//! kill leaves a database a later pass completes, and the OS frees the
//! lock. No file-event crate is used, and nothing is spawned.

use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use phantom_fractal_extract::{Freshness, probe};
use phantom_fractal_git::Window;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{Store, index_database_path};
use serde::Serialize;

use crate::ops::cochange::stored_window;
use crate::ops::index::{CochangeSummary, checked_root, open_locked, pass};
use crate::ops::{Failure, Reply, envelope, fresh};

/// `--interval-ms`: how often the tree is probed.
pub(crate) const DEFAULT_INTERVAL_MS: u64 = 500;
pub(crate) const INTERVAL_MS_MIN: u64 = 100;
pub(crate) const INTERVAL_MS_MAX: u64 = 10_000;

/// `--debounce-ms`: how long a stale tree must hold still.
pub(crate) const DEFAULT_DEBOUNCE_MS: u64 = 250;
pub(crate) const DEBOUNCE_MS_MIN: u64 = 50;
pub(crate) const DEBOUNCE_MS_MAX: u64 = 5_000;

/// `--max-batches`: how many passes to run before exiting 0.
pub(crate) const MAX_BATCHES_MIN: u64 = 1;
pub(crate) const MAX_BATCHES_MAX: u64 = 1_000_000;

/// Canon exit 1, for a batch line that could not be printed.
const EXIT_ERROR: u8 = 1;

/// What `watch` is asked for.
pub(crate) struct WatchRequest {
    /// The tree to keep indexed.
    pub(crate) repo: PathBuf,
    pub(crate) interval: Duration,
    pub(crate) debounce: Duration,
    /// Exit 0 after this many batches; run until killed when none.
    pub(crate) max_batches: Option<u64>,
}

/// `data` of one batch line, with the names `index` uses. It holds counts
/// only, never a path.
#[derive(Serialize)]
pub(crate) struct Batch {
    /// 1 for the first batch, counting up.
    batch: u64,
    /// `.rs` files the pass wrote.
    written: usize,
    /// Stored paths the pass removed.
    removed: usize,
    /// Whether the pass rebuilt the crate graph and edges.
    rebuilt: bool,
    /// The stored co-change scan after the pass, on a git root only.
    cochange: Option<CochangeSummary>,
}

/// Runs `watch` and prints its lines. It exits 0 after `max_batches`
/// batches, or with a failure's code after printing its envelope.
pub(crate) fn run(request: &WatchRequest) -> ExitCode {
    let result = watch(request, &mut |reply| match envelope(Ok(reply)) {
        (Ok(line), _) if print(&line) => Ok(()),
        _ => Err(()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop::Output) => ExitCode::from(EXIT_ERROR),
        Err(Stop::Failed(failure)) => match envelope::<Batch>(Err(failure)) {
            (Ok(line), code) if print(&line) => {
                ExitCode::from(u8::try_from(code).unwrap_or(EXIT_ERROR))
            }
            _ => ExitCode::from(EXIT_ERROR),
        },
    }
}

/// Writes `line` and a newline to stdout and flushes, so a reader sees
/// each batch as it ends. False when stdout is gone.
fn print(line: &str) -> bool {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").and_then(|()| out.flush()).is_ok()
}

/// Why [`watch`] returned early.
enum Stop {
    /// A probe or a pass failed twice in a row, or the start failed.
    Failed(Failure),
    /// A batch line could not be written: the reader is gone.
    Output,
}

impl From<Failure> for Stop {
    fn from(failure: Failure) -> Self {
        Self::Failed(failure)
    }
}

/// The loop behind [`run`]. `emit` takes each batch's reply.
///
/// A probe or pass that fails is tried again at the next interval: a save
/// can remove a file between the walk and the read. A second failure in a
/// row ends the watch with that failure.
fn watch(
    request: &WatchRequest,
    emit: &mut dyn FnMut(Reply<Batch>) -> Result<(), ()>,
) -> Result<(), Stop> {
    let root = checked_root(&request.repo)?;
    let language = RustLanguage::new()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    let (mut store, _lock) = open_locked(&index_database_path(&root))?;
    let mut batches = 0;
    let mut failed = false;
    let mut first = true;
    loop {
        if !std::mem::take(&mut first) {
            sleep(request.interval);
        }
        let step = settle(&root, &store, request.debounce).and_then(|stale| {
            if !stale {
                return Ok(None);
            }
            let window = window(&store)?;
            pass(&root, &mut store, &language, window).map(Some)
        });
        let done = match step {
            Ok(done) => {
                failed = false;
                done
            }
            Err(_) if !failed => {
                failed = true;
                continue;
            }
            Err(failure) => return Err(failure.into()),
        };
        let Some(done) = done else {
            continue;
        };
        batches += 1;
        let meta = fresh::meta(&root, &store)?;
        let batch = Batch {
            batch: batches,
            written: done.report.written.len(),
            removed: done.report.removed.len(),
            rebuilt: done.report.graph.is_some(),
            cochange: done.cochange,
        };
        emit(Reply::with_meta(batch, meta)).map_err(|()| Stop::Output)?;
        if request.max_batches.is_some_and(|max| batches >= max) {
            return Ok(());
        }
    }
}

/// True once the tree is stale and has settled, false when it is fresh:
/// after a stale probe, it waits `debounce` and probes again until two
/// probes in a row give the same [`Look`].
fn settle(root: &Path, store: &Store, debounce: Duration) -> Result<bool, Failure> {
    let mut look = Look::new(root, store)?;
    while look.freshness.stale() {
        sleep(debounce);
        let next = Look::new(root, store)?;
        if next == look {
            return Ok(true);
        }
        look = next;
    }
    Ok(false)
}

/// One probe, plus the size and mtime of each file it lists as added or
/// modified. Two looks are equal only when the probe is equal and none of
/// those files moved, so a file saved again and again inside the debounce
/// is not taken as settled after its first save.
#[derive(PartialEq, Eq)]
struct Look {
    freshness: Freshness,
    stats: Vec<Option<(u64, i64, i64)>>,
}

impl Look {
    fn new(root: &Path, store: &Store) -> Result<Self, Failure> {
        let freshness = probe(root, store)?;
        let stats = freshness
            .added
            .iter()
            .chain(&freshness.modified)
            .map(|path| {
                fs::symlink_metadata(root.join(path))
                    .ok()
                    .map(|meta| (meta.len(), meta.mtime(), meta.mtime_nsec()))
            })
            .collect();
        Ok(Self { freshness, stats })
    }
}

/// The window of the stored co-change scan, so a pass keeps the window
/// `index` was given; the default when no scan is stored.
fn window(store: &Store) -> Result<Window, Failure> {
    match store.cochange_scan()? {
        Some(scan) if scan.last_oid.as_deref().is_some_and(|oid| !oid.is_empty()) => {
            stored_window(&scan.window_json)
        }
        _ => Ok(Window::default()),
    }
}
