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

use std::collections::BTreeSet;

use phantom_fractal_extract::{
    Freshness, Registry, default_registry, index_with, probe_with, stored_graph_with,
};
use phantom_fractal_git::Window;
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{CochangeScan, Store, index_database_path};
use serde::Serialize;

use crate::ops::cochange::stored_window;
use crate::ops::index::{CochangeSummary, Pass, checked_root, open_locked, stored_cochange};
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
    let registry = default_registry()
        .map_err(|err| Failure::new(ErrorKind::Usage, format!("extract: {err}")))?;
    watch_with(request, &registry, emit)
}

fn watch_with(
    request: &WatchRequest,
    registry: &Registry<'_>,
    emit: &mut dyn FnMut(Reply<Batch>) -> Result<(), ()>,
) -> Result<(), Stop> {
    let root = checked_root(&request.repo)?;
    let (store, lock) = open_locked(&index_database_path(&root))?;
    // The lock is bound before the store, so when the watch ends the store
    // is closed first and the lock is released after it, as in `index`.
    let _lock = lock;
    let mut store = store;
    watch_loop(&root, &mut store, request, registry, emit)
}

fn watch_loop(
    root: &Path,
    store: &mut Store,
    request: &WatchRequest,
    registry: &Registry<'_>,
    emit: &mut dyn FnMut(Reply<Batch>) -> Result<(), ()>,
) -> Result<(), Stop> {
    let mut batches = 0;
    let mut failed = false;
    let mut first = true;
    loop {
        let Some(done) = wait_and_step(root, store, request, registry, &mut failed, &mut first)?
        else {
            continue;
        };
        batches += 1;
        emit_batch(root, store, batches, done, emit)?;
        if request.max_batches.is_some_and(|max| batches >= max) {
            return Ok(());
        }
    }
}

fn wait_and_step(
    root: &Path,
    store: &mut Store,
    request: &WatchRequest,
    registry: &Registry<'_>,
    failed: &mut bool,
    first: &mut bool,
) -> Result<Option<Pass>, Stop> {
    if !std::mem::take(first) {
        sleep(request.interval);
    }
    try_watch_step(root, store, registry, request.debounce, failed)
}

fn try_watch_step(
    root: &Path,
    store: &mut Store,
    registry: &Registry<'_>,
    debounce: Duration,
    failed: &mut bool,
) -> Result<Option<Pass>, Stop> {
    match run_step(root, store, registry, debounce) {
        Ok(done) => {
            *failed = false;
            Ok(done)
        }
        Err(_) if !*failed => {
            *failed = true;
            Ok(None)
        }
        Err(failure) => Err(failure.into()),
    }
}

fn emit_batch(
    root: &Path,
    store: &Store,
    batch_num: u64,
    done: Pass,
    emit: &mut dyn FnMut(Reply<Batch>) -> Result<(), ()>,
) -> Result<(), Stop> {
    let meta = fresh::meta(root, store)?;
    let batch = Batch {
        batch: batch_num,
        written: done.report.written.len(),
        removed: done.report.removed.len(),
        rebuilt: done.report.graph.is_some(),
        cochange: done.cochange,
    };
    emit(Reply::with_meta(batch, meta)).map_err(|()| Stop::Output)
}

/// True once the tree is stale and has settled, false when it is fresh.
fn settle(
    root: &Path,
    store: &Store,
    registry: &Registry<'_>,
    debounce: Duration,
) -> Result<bool, Failure> {
    let mut look = Look::new(root, store, registry)?;
    while look.freshness.stale() {
        sleep(debounce);
        let next = Look::new(root, store, registry)?;
        if next == look {
            return Ok(true);
        }
        look = next;
    }
    Ok(false)
}

#[derive(PartialEq, Eq)]
struct Look {
    freshness: Freshness,
    stats: Vec<Option<(u64, i64, i64)>>,
}

impl Look {
    fn new(root: &Path, store: &Store, registry: &Registry<'_>) -> Result<Self, Failure> {
        let freshness = probe_with(root, store, registry)?;
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

fn window(store: &Store) -> Result<Window, Failure> {
    match store.cochange_scan()? {
        Some(scan) if scan.last_oid.as_deref().is_some_and(|oid| !oid.is_empty()) => {
            stored_window(&scan.window_json)
        }
        _ => Ok(Window::default()),
    }
}

fn run_step(
    root: &Path,
    store: &mut Store,
    registry: &Registry<'_>,
    debounce: Duration,
) -> Result<Option<Pass>, Failure> {
    if !settle(root, store, registry, debounce)? {
        return Ok(None);
    }
    store
        .atomic_batch(|store| pass_with(root, store, registry, window(store)?))
        .map(Some)
}

pub(crate) fn pass_with(
    root: &Path,
    store: &mut Store,
    registry: &Registry<'_>,
    window: Window,
) -> Result<Pass, Failure> {
    let report = index_with(root, store, registry)?;
    let graph = report_graph(&report, store, root, registry)?;
    let head = read_head(root)?;
    let cochange = finish_pass(store, root, head.as_deref(), window)?;
    Ok(Pass {
        report,
        graph,
        cochange,
    })
}

fn report_graph(
    report: &phantom_fractal_extract::IndexReport,
    store: &Store,
    root: &Path,
    registry: &Registry<'_>,
) -> Result<phantom_fractal_extract::GraphReport, Failure> {
    match report.graph {
        Some(graph) => Ok(graph),
        None => Ok(stored_graph_with(store, root, registry)?),
    }
}

fn finish_pass(
    store: &mut Store,
    root: &Path,
    head: Option<&str>,
    window: Window,
) -> Result<Option<CochangeSummary>, Failure> {
    update_rank(store)?;
    let cochange = update_cochange(store, root, head, window)?;
    store.set_root(&root.to_string_lossy())?;
    store.set_index_head(head)?;
    Ok(cochange)
}

fn read_head(root: &Path) -> Result<Option<String>, Failure> {
    phantom_fractal_git::head(root).map_err(|_| git_history_failure())
}

fn git_history_failure() -> Failure {
    Failure::new(
        ErrorKind::Usage,
        "git history could not be read; the files were indexed, the co-change was not",
    )
}

fn update_rank(store: &mut Store) -> Result<(), Failure> {
    if store.rank_epoch()? != store.graph_epoch()? {
        let ranks = phantom_fractal_analyze::rank(&*store)?;
        store.write_rank(ranks.epoch, &ranks.symbols, &ranks.files)?;
    }
    Ok(())
}

fn update_cochange(
    store: &mut Store,
    root: &Path,
    head: Option<&str>,
    window: Window,
) -> Result<Option<CochangeSummary>, Failure> {
    match head {
        None => clear_stale_cochange(store),
        Some(oid) => refresh_git_cochange(store, root, oid, window),
    }
}

fn clear_stale_cochange(store: &mut Store) -> Result<Option<CochangeSummary>, Failure> {
    if store.cochange_head()?.is_some() {
        store.write_cochange(&CochangeScan {
            last_oid: Some(String::new()),
            ..CochangeScan::default()
        })?;
    }
    Ok(None)
}

fn refresh_git_cochange(
    store: &mut Store,
    root: &Path,
    head: &str,
    window: Window,
) -> Result<Option<CochangeSummary>, Failure> {
    if !is_cochange_fresh(store, head, &window)? {
        mine_and_store_cochange(store, root, &window)?;
    }
    stored_cochange(store)
}

fn mine_and_store_cochange(store: &mut Store, root: &Path, window: &Window) -> Result<(), Failure> {
    let indexed: BTreeSet<String> = store.files()?.into_iter().map(|f| f.path).collect();
    if let Some(scan) =
        phantom_fractal_git::mine(root, window, &indexed).map_err(|_| git_history_failure())?
    {
        store.write_cochange(&scan)?;
    }
    Ok(())
}

fn is_cochange_fresh(store: &Store, head: &str, window: &Window) -> Result<bool, Failure> {
    let window_json = window.to_json();
    Ok(store.cochange_scan()?.is_some_and(|scan| {
        scan.last_oid.as_deref() == Some(head) && scan.window_json == window_json
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use phantom_fractal_git::Window;
    use phantom_fractal_lang_rust::RustLanguage;
    use phantom_fractal_store::{Store, WriterLock};

    use crate::ops::index::pass;

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "phantom-watch-unit-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("dir");
            Self(path.canonicalize().expect("canon"))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn pass_with_dispatches_non_rust_registry_cleanly() {
        use phantom_fractal_extract::{
            LanguageDriver, ManifestFile, PackageGraph, Registry, SourceFileExtract,
        };
        use phantom_fractal_lang::{
            Edge, Extract, ExtractError, FileModule, Item, ItemKind, Language, Module,
            ResolveCounts, ResolveInput, Resolver, Target, TargetKind, Visibility,
        };
        use phantom_fractal_store::Package;

        struct TinyLang;
        impl Language for TinyLang {
            fn name(&self) -> &'static str {
                "tiny"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &["tiny"]
            }
            fn extract(&self, _s: &str) -> Result<Extract, ExtractError> {
                Ok(Extract {
                    items: vec![Item {
                        kind: ItemKind::Function,
                        name: "entry".to_owned(),
                        visibility: Visibility::Public,
                        start_line: 1,
                        end_line: 1,
                        signature: "fn entry".to_owned(),
                        doc_summary: None,
                        is_test: false,
                    }],
                    item_scopes: vec![String::new()],
                    item_owners: vec![None],
                    ..Extract::default()
                })
            }
        }
        struct TinyResolver;
        impl Resolver for TinyResolver {
            fn name(&self) -> &'static str {
                "tiny"
            }
            fn resolve(&self, _input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
                (Vec::new(), ResolveCounts::default())
            }
        }
        fn tiny_prune(_p: &std::path::Path, _n: &str, _d: usize) -> bool {
            false
        }
        struct TinyDriver(TinyLang, TinyResolver);
        impl LanguageDriver for TinyDriver {
            fn language(&self) -> &dyn Language {
                &self.0
            }
            fn resolver(&self) -> &dyn Resolver {
                &self.1
            }
            fn manifest_names(&self) -> &'static [&'static str] {
                &["tiny.mod"]
            }
            fn prune_rule(&self) -> fn(&std::path::Path, &str, usize) -> bool {
                tiny_prune
            }
            fn build_graph(
                &self,
                _m: &[ManifestFile],
                files: &[SourceFileExtract],
            ) -> PackageGraph {
                PackageGraph {
                    packages: vec![Package {
                        name: "tiny".to_owned(),
                        version: None,
                        manifest_path: "tiny.mod".to_owned(),
                    }],
                    targets: vec![Target {
                        id: 1,
                        package: "tiny".to_owned(),
                        name: "pkg".to_owned(),
                        kind: TargetKind::Lib,
                        root: String::new(),
                    }],
                    modules: vec![Module {
                        id: 1,
                        target: 1,
                        parent: None,
                        path: "tiny/pkg".to_owned(),
                        file: String::new(),
                        scope: String::new(),
                    }],
                    file_modules: files
                        .iter()
                        .map(|f| FileModule {
                            file: f.path.clone(),
                            module: 1,
                        })
                        .collect(),
                    test_modules: Vec::new(),
                    orphans: 0,
                }
            }
        }

        let tree = TempDir::new("tiny-tree");
        let data = TempDir::new("tiny-data");
        fs::write(tree.0.join("tiny.mod"), "tiny\n").unwrap();
        fs::write(tree.0.join("a.tiny"), "fn entry\n").unwrap();
        let db = data.0.join("repos/tiny/graph.db");
        let _lock = WriterLock::acquire(&db).unwrap();
        let mut store = Store::open(&db).unwrap();
        let driver = TinyDriver(TinyLang, TinyResolver);
        let registry = Registry::new(vec![&driver]);
        let Ok(passed) =
            store.atomic_batch(|s| super::pass_with(&tree.0, s, &registry, Window::default()))
        else {
            panic!("pass_with failed");
        };
        assert_eq!(passed.report.written, vec!["a.tiny"]);
        assert!(passed.report.graph.is_some());
        assert_eq!(store.symbols_matching("entry").unwrap().len(), 1);

        // Git repo test: populates cochange, skips re-mining when fresh, re-mines on window change,
        // and clears cochange if .git is removed (last_oid -> Some("")).
        let git_tree = TempDir::new("tiny-git");
        let git_home = TempDir::new("tiny-home");
        fs::write(git_tree.0.join("tiny.mod"), "tiny\n").unwrap();
        fs::write(git_tree.0.join("a.tiny"), "fn entry\n").unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["add", "."],
            &["commit", "-q", "-m", "init"],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&git_tree.0)
                .env("HOME", &git_home.0)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@e.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@e.com")
                .status()
                .unwrap();
            assert!(status.success());
        }
        let gdb = data.0.join("repos/tiny-git/graph.db");
        let _glock = WriterLock::acquire(&gdb).unwrap();
        let mut gstore = Store::open(&gdb).unwrap();
        let Ok(p1) =
            gstore.atomic_batch(|s| super::pass_with(&git_tree.0, s, &registry, Window::default()))
        else {
            panic!("git pass 1 failed");
        };
        assert!(p1.cochange.is_some());
        assert!(p1.graph.modules > 0);
        // Seed a sentinel commit count with matching head+window so p_noop proves
        // is_cochange_fresh skips mining while p2 (new window) re-mines.
        let mut seeded = gstore.cochange_scan().unwrap().unwrap();
        seeded.commits_seen = 77;
        gstore.write_cochange(&seeded).unwrap();
        let Ok(p_noop) =
            gstore.atomic_batch(|s| super::pass_with(&git_tree.0, s, &registry, Window::default()))
        else {
            panic!("git noop failed");
        };
        assert!(p_noop.report.graph.is_none() && p_noop.cochange.is_some());
        assert_eq!(p_noop.graph.modules, p1.graph.modules);
        assert_eq!(gstore.cochange_scan().unwrap().unwrap().commits_seen, 77);
        let req_long = super::WatchRequest {
            repo: git_tree.0.clone(),
            interval: std::time::Duration::from_secs(30),
            debounce: std::time::Duration::from_millis(1),
            max_batches: Some(1),
        };
        let mut first = true;
        let mut tick_failed = false;
        let t0 = std::time::Instant::now();
        let _ = super::wait_and_step(
            &git_tree.0,
            &mut gstore,
            &req_long,
            &registry,
            &mut tick_failed,
            &mut first,
        )
        .ok()
        .unwrap();
        assert!(!first);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
        let req_short = super::WatchRequest {
            interval: std::time::Duration::from_millis(25),
            ..req_long
        };
        let t1 = std::time::Instant::now();
        let _ = super::wait_and_step(
            &git_tree.0,
            &mut gstore,
            &req_short,
            &registry,
            &mut tick_failed,
            &mut first,
        )
        .ok()
        .unwrap();
        assert!(t1.elapsed() >= std::time::Duration::from_millis(20));
        let w2 = Window {
            days: 30,
            max_commits: 100,
        };
        let Ok(p2) = gstore.atomic_batch(|s| super::pass_with(&git_tree.0, s, &registry, w2))
        else {
            panic!("git pass 2 failed");
        };
        assert!(p2.cochange.is_some());
        assert_eq!(gstore.cochange_scan().unwrap().unwrap().commits_seen, 1);
        // Exercise try_watch_step consecutive failure handling cleanly.
        let mut failed = false;
        assert!(matches!(
            super::try_watch_step(
                std::path::Path::new("/nonexistent-phantom-watch-root"),
                &mut gstore,
                &registry,
                std::time::Duration::from_millis(1),
                &mut failed,
            ),
            Ok(None)
        ));
        assert!(failed);
        assert!(
            super::try_watch_step(
                std::path::Path::new("/nonexistent-phantom-watch-root"),
                &mut gstore,
                &registry,
                std::time::Duration::from_millis(1),
                &mut failed,
            )
            .is_err()
        );
        // Removing .git clears stored cochange (last_oid = Some("") -> cochange_head None).
        fs::remove_dir_all(git_tree.0.join(".git")).unwrap();
        let Ok(p3) = gstore.atomic_batch(|s| super::pass_with(&git_tree.0, s, &registry, w2))
        else {
            panic!("git pass 3 failed");
        };
        assert!(p3.cochange.is_none());
        assert!(super::mine_and_store_cochange(&mut gstore, &git_tree.0, &w2).is_ok());
        assert_eq!(gstore.cochange_head().unwrap(), None);
        assert_eq!(
            gstore.cochange_scan().unwrap().unwrap().last_oid.as_deref(),
            Some("")
        );
    }

    #[test]
    fn mid_batch_interruption_with_barrier_rolls_back_and_index_catches_all() {
        let tree = TempDir::new("tree");
        let data = TempDir::new("data");
        fs::create_dir_all(tree.0.join("src")).expect("src");
        fs::write(
            tree.0.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("toml");
        fs::write(tree.0.join("src/lib.rs"), "pub fn init() {}\n").expect("lib");

        // Initial index completes batch 1.
        let db = data.0.join("repos/app/graph.db");
        let lang = RustLanguage::new().expect("lang");
        {
            let _lock = WriterLock::acquire(&db).expect("lock");
            let mut store = Store::open(&db).expect("open");
            let batch1 = store.atomic_batch(|s| pass(&tree.0, s, &lang, Window::default()));
            assert!(batch1.is_ok(), "batch 1 failed");
        }

        // Write 30 new files while watch is active.
        for n in 0..30 {
            fs::write(
                tree.0.join(format!("src/gen_{n}.rs")),
                format!("pub fn gen_{n}() {{}}\n"),
            )
            .expect("gen file");
        }

        // Deterministic Barrier-synchronized mid-batch interruption (no sleep).
        let mid_write = Arc::new(Barrier::new(2));
        let aborted = Arc::new(Barrier::new(2));
        let mid_c = Arc::clone(&mid_write);
        let abort_c = Arc::clone(&aborted);
        let db_c = db.clone();
        let root_c = tree.0.clone();

        let handle = thread::spawn(move || {
            let _lock = WriterLock::acquire(&db_c).expect("lock");
            let mut store = Store::open(&db_c).expect("open");
            let lang = RustLanguage::new().expect("lang");
            let res: Result<(), crate::ops::Failure> = store.atomic_batch(|s| {
                let _ = phantom_fractal_extract::index(&root_c, s, &lang)?;
                mid_c.wait();
                abort_c.wait();
                Err(crate::ops::Failure::new(
                    phantom_fractal_proto::ErrorKind::Usage,
                    "simulated mid-batch kill",
                ))
            });
            assert!(res.is_err());
        });

        mid_write.wait();
        aborted.wait();
        handle.join().expect("join");

        // Nothing from the aborted batch leaked into files.
        let store = Store::open(&db).expect("reopen");
        assert_eq!(store.file_paths().expect("paths"), vec!["src/lib.rs"]);
        drop(store);

        // Subsequent index catches all 30 files cleanly.
        let fresh_data = TempDir::new("fresh");

        let mut recovered = Store::open(&db).expect("open recovered");
        let recovered_pass = pass(&tree.0, &mut recovered, &lang, Window::default());
        assert!(recovered_pass.is_ok(), "recovered pass failed");
        assert_eq!(recovered.file_paths().expect("all").len(), 31);
        let mut fresh = Store::open(&fresh_data.0.join("graph.db")).expect("fresh");
        let fresh_pass = pass(&tree.0, &mut fresh, &lang, Window::default());
        assert!(fresh_pass.is_ok(), "fresh pass failed");
        assert_eq!(
            recovered.canonical_dump().expect("dump"),
            fresh.canonical_dump().expect("fresh dump")
        );
    }
}
