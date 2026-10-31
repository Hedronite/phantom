//! The writer lock and its state probe (W9 D2): the file the acquire
//! creates, the two contention cases, a killed holder, and a probe
//! that never creates the file.
//!
//! The two-process spawn re-runs this test binary with a single gated
//! child test and a lock path in the environment; the child holds the
//! lock until the parent kills it. Only tests spawn processes here.

use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{TempData, TempDb, assert_outside_workspace, file_mode};
use crate::{LockState, Store, StoreError, WriterLock, lock_state};

/// The test's own process runs the child.
fn spawn_lock_child(db: &std::path::Path, marker: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args([
            "tests::lock::spawned_child_holds_the_lock",
            "--exact",
            "--nocapture",
        ])
        .env("PHA_STORE_TEST_LOCK_DB", db)
        .env("PHA_STORE_TEST_LOCK_MARKER", marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Starts the child and answers once its marker shows the lock held.
fn wait_for_held(child: &mut std::process::Child, marker: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait().expect("child status").is_some() {
            panic!("child exited before the marker showed");
        }
        if std::fs::metadata(marker).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("child never reported the lock");
}

#[test]
fn acquire_creates_an_empty_0600_file_and_releases_on_drop() {
    let db = TempDb::new("lockfile");
    assert_outside_workspace(db.path());
    let lock_path = crate::lock::lock_path(db.path());
    assert!(matches!(lock_state(db.path()), Ok(LockState::Absent)));
    assert!(!lock_path.exists(), "the probe created no lock file");

    let guard = WriterLock::acquire(db.path()).expect("acquire");
    let mut text = String::new();
    std::fs::OpenOptions::new()
        .read(true)
        .open(&lock_path)
        .map(|mut file| file.read_to_string(&mut text).expect("lock read"))
        .expect("lock open");
    assert_eq!(text, String::new(), "the lock file holds content");
    assert_eq!(file_mode(&lock_path), 0o600);
    // A probe from a second handle answers `Held` while the writer
    // holds, and `Free` again once the guard drops.
    assert!(matches!(lock_state(db.path()), Ok(LockState::Held)));
    drop(guard);
    assert!(matches!(lock_state(db.path()), Ok(LockState::Free)));
    let _ = std::fs::remove_file(&lock_path);
}

#[test]
fn two_threads_contend_and_the_loser_gets_lock_held() {
    let db = TempDb::new("lock2thread");
    assert_outside_workspace(db.path());
    // The chain is born outside the timed window on a loaded runner:
    // a warm first acquire makes both contenders' `acquire` runs start
    // from an existing chain, so the timing below measures only the
    // contention itself.
    let warm = WriterLock::acquire(db.path()).expect("warm");
    drop(warm);
    let first = WriterLock::acquire(db.path()).expect("first");
    let path = db.path().to_path_buf();
    let loser = std::thread::spawn(move || {
        let started = Instant::now();
        let result = WriterLock::acquire(&path);
        (result, started.elapsed())
    });
    let (result, elapsed) = loser.join().expect("join");
    assert!(matches!(result, Err(StoreError::LockHeld)), "{result:?}");
    // The lower bound proves the full 250 ms retry window was waited;
    // the upper bound is CI slack for a loaded runner (the 250 + 50 ms
    // figure is measured by QA on the build box).
    assert!(
        elapsed >= crate::lock::RETRY_WINDOW,
        "LockHeld answered after {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_secs(2),
        "LockHeld answered after {elapsed:?}"
    );
    drop(first);
    let again = WriterLock::acquire(db.path()).expect("free after the holder drops");
    drop(again);
    let _ = std::fs::remove_file(crate::lock::lock_path(db.path()));
}

#[test]
fn two_processes_contend_and_the_loser_gets_lock_held() {
    let db = TempDb::new("lock2proc");
    assert_outside_workspace(db.path());
    let marker = std::env::temp_dir().join(format!(
        "phantom-fractal-store-marker-{}-{}",
        std::process::id(),
        db.path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&marker);
    let mut child = spawn_lock_child(db.path(), &marker).spawn().expect("spawn");
    wait_for_held(&mut child, &marker);
    // A warm untimed acquire gives both sides their chain before the
    // timed window, as its own proof of contention the child holds.
    match WriterLock::acquire(db.path()) {
        Err(StoreError::LockHeld) => {}
        other => panic!("the warm contend missed the held lock: {other:?}"),
    }
    let started = Instant::now();
    let result = WriterLock::acquire(db.path());
    assert!(matches!(result, Err(StoreError::LockHeld)), "{result:?}");
    // The lower bound proves the full 250 ms retry window was waited;
    // the upper bound is CI slack for a loaded runner (the 250 + 50 ms
    // figure is measured by QA on the build box).
    let elapsed = started.elapsed();
    assert!(elapsed >= crate::lock::RETRY_WINDOW, "{elapsed:?}");
    assert!(
        elapsed <= Duration::from_secs(2),
        "LockHeld answered after {elapsed:?}"
    );
    child.kill().expect("kill child");
    let _ = child.wait();
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(crate::lock::lock_path(db.path()));
}

#[test]
fn killed_holder_frees_the_lock() {
    let db = TempDb::new("lockkill");
    assert_outside_workspace(db.path());
    let marker = std::env::temp_dir().join(format!(
        "phantom-fractal-store-marker-{}-{}",
        std::process::id(),
        db.path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&marker);
    let mut child = spawn_lock_child(db.path(), &marker).spawn().expect("spawn");
    wait_for_held(&mut child, &marker);
    assert!(matches!(lock_state(db.path()), Ok(LockState::Held)));
    child.kill().expect("kill child");
    let _ = child.wait();
    // The OS freed the lock on the kill: a new writer takes it at once.
    assert!(matches!(lock_state(db.path()), Ok(LockState::Free)));
    let again = WriterLock::acquire(db.path()).expect("free after kill");
    drop(again);
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(crate::lock::lock_path(db.path()));
}

#[test]
fn lock_state_never_creates_the_file() {
    let db = TempDb::new("lockstate");
    assert_outside_workspace(db.path());
    let lock_path = crate::lock::lock_path(db.path());
    assert!(matches!(lock_state(db.path()), Ok(LockState::Absent)));
    assert!(!lock_path.exists(), "a probe created the lock file");
    assert!(matches!(lock_state(db.path()), Ok(LockState::Absent)));
    assert!(!lock_path.exists(), "a probe created the lock file");
}

/// The gated child the two-process tests spawn: holds the lock at the
/// path the environment names, reports via the marker file, and stays
/// held until the parent kills this whole process. A bare run without
/// the environment answers at once.
#[test]
fn spawned_child_holds_the_lock() {
    let Ok(db) = std::env::var("PHA_STORE_TEST_LOCK_DB") else {
        return;
    };
    let Ok(marker) = std::env::var("PHA_STORE_TEST_LOCK_MARKER") else {
        return;
    };
    let _guard = WriterLock::acquire(std::path::Path::new(&db)).expect("child acquire");
    std::fs::write(&marker, "held").expect("marker");
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn acquire_creates_the_0700_chain_and_holds() {
    let data = TempData::new("lockchain");
    // No chain exists yet: only the lock file is here, and the chain
    // Store::open would have created is born around it — at `0700`,
    // before any database file exists at all.
    let guard = WriterLock::acquire(&data.db()).expect("acquire");
    assert_outside_workspace(&data.db());
    assert_eq!(file_mode(&data.root.join("repos")), 0o700);
    assert_eq!(file_mode(&data.repo_dir()), 0o700);
    let lock = crate::lock::lock_path(&data.db());
    assert_eq!(file_mode(&lock), 0o600);
    let mut text = String::new();
    std::fs::OpenOptions::new()
        .read(true)
        .open(&lock)
        .map(|mut file| file.read_to_string(&mut text).expect("lock read"))
        .expect("lock open");
    assert_eq!(text, String::new(), "the lock file holds content");
    assert!(matches!(lock_state(&data.db()), Ok(LockState::Held)));
    drop(guard);
    // The chain the acquire made is the chain the store opens.
    let store = Store::open(&data.db()).expect("store on the born chain");
    assert_eq!(store.row_count("meta").expect("rows"), 2);
    drop(store);
    let _ = std::fs::remove_file(&lock);
}

#[test]
fn acquire_refuses_a_broken_chain_like_the_store() {
    use std::fs;
    let data = TempData::new("lockshape");
    fs::create_dir_all(data.root.join("repos")).expect("repos");
    // The index dir is a file: the shape refusal, no chmod anywhere.
    fs::File::create(data.repo_dir()).expect("file the repo spot");
    let name_err = |err: StoreError| format!("{err:?}");
    let acquired = WriterLock::acquire(&data.db())
        .map(|_| ())
        .map_err(name_err);
    let opened = Store::open(&data.db()).map(|_| ()).map_err(name_err);
    // The same refusal, from the same checks.
    assert_eq!(acquired, opened, "{acquired:?}");
    let refused = acquired.as_ref().expect_err("the file-refused dir");
    assert!(refused.contains("DataDirectory"), "{refused}");
    // and neither left a database, nor content: the refusal lands
    // before any lock file is opened.
    assert!(!crate::lock::lock_path(&data.db()).exists());
    // A symlinked repos/ refrains the same way.
    let data = TempData::new("locklink");
    fs::create_dir_all(&data.root).expect("data root");
    let elsewhere = TempData::new("locklink-target");
    fs::create_dir_all(elsewhere.repo_dir()).expect("target repo");
    std::os::unix::fs::symlink(elsewhere.repo_dir(), data.root.join("repos")).expect("symlink");
    let acquired = WriterLock::acquire(&data.db())
        .map(|_| ())
        .map_err(name_err);
    let opened = Store::open(&data.db()).map(|_| ()).map_err(name_err);
    assert_eq!(acquired, opened, "{acquired:?}");
    let refused = acquired.as_ref().expect_err("the symlinked dir");
    assert!(refused.contains("DataDirectory"), "{refused}");
}

/// The store's refusal for a non-regular or swapped lock file, named
/// by role and never by path.
fn assert_lock_refusal(err: &StoreError, detail: &str) {
    match err {
        StoreError::DataDirectory(role, text) => {
            assert_eq!(role, "lock file");
            assert!(text.contains(detail), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

/// A repo dir with no database, for a bare lock path to meet.
fn bare_repo(label: &str) -> (TempData, std::path::PathBuf) {
    let data = TempData::new(label);
    std::fs::create_dir_all(data.repo_dir()).expect("repo dir");
    let lock = crate::lock::lock_path(&data.db());
    (data, lock)
}

/// The (mtime, mode, bytes) of one plain file, for the not-touched
/// checks.
fn file_state(path: &std::path::Path) -> ((i64, i64), u32, Vec<u8>) {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).expect("state");
    (
        (meta.mtime(), meta.mtime_nsec()),
        file_mode(path),
        fs::read(path).expect("bytes"),
    )
}

#[test]
fn acquire_refuses_a_symlink_and_spares_its_target() {
    let (data, lock) = bare_repo("roloclink");
    let target = data.repo_dir().join("scout");
    std::fs::write(&target, b"the target bytes").expect("target");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .expect("target mode");
    }
    std::os::unix::fs::symlink("scout", &lock).expect("symlink");
    let before = file_state(&target);

    let refused = WriterLock::acquire(&data.db()).expect_err("refused");
    assert_lock_refusal(&refused, "symlink or not a regular file");
    let probed = lock_state(&data.db()).expect_err("refused");
    assert_lock_refusal(&probed, "symlink or not a regular file");

    // The target is untouched: bytes, mode, and mtime, and the link is
    // still a link.
    assert_eq!(before, file_state(&target));
    let meta = std::fs::symlink_metadata(&lock).expect("link stays");
    assert!(meta.file_type().is_symlink(), "{meta:?}");
}

#[test]
fn acquire_and_lock_state_refuse_a_dangling_symlink() {
    let (data, lock) = bare_repo("roloclink-dg");
    std::os::unix::fs::symlink("nowhere", &lock).expect("dangling");

    let refused = WriterLock::acquire(&data.db()).expect_err("refused");
    assert_lock_refusal(&refused, "symlink or not a regular file");
    assert!(
        matches!(lock_state(&data.db()), Err(StoreError::DataDirectory(_, _))),
        "lock_state answered a dangling link with store refusal"
    );

    // The dangling target still does not exist, and only the link is
    // there — nothing was created beyond the refused link itself.
    assert!(!data.repo_dir().join("nowhere").exists());
    let mut left: Vec<String> = std::fs::read_dir(data.repo_dir())
        .expect("repo dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, vec!["graph.db.lock".to_owned()], "{left:?}");
}

#[test]
fn acquire_refuses_a_directory_at_the_lock_path() {
    let (data, lock) = bare_repo("rolocdir");
    std::fs::create_dir(&lock).expect("dir at the lock path");
    let refused = WriterLock::acquire(&data.db()).expect_err("refused");
    assert_lock_refusal(&refused, "symlink or not a regular file");
    let probed = lock_state(&data.db()).expect_err("refused");
    assert_lock_refusal(&probed, "symlink or not a regular file");
    // Refused without touching the directory's content.
    assert!(std::fs::read_dir(&lock).is_ok(), "the dir was disturbed");
}

#[test]
fn lock_state_refuses_a_directory_and_a_link_to_one() {
    // The FIFO case takes the same `is_file()` false a directory or a
    // link to one takes: both are non-regular entries at the lock
    // path, and QA plants a real FIFO against the same refusal.
    let (data, lock) = bare_repo("rolocdir2");
    let target = data.repo_dir().join("somedir");
    std::fs::create_dir(&target).expect("target dir");
    std::os::unix::fs::symlink(&target, &lock).expect("dir link");

    let refused = WriterLock::acquire(&data.db()).expect_err("refused");
    assert_lock_refusal(&refused, "symlink or not a regular file");
    let probed = lock_state(&data.db()).expect_err("refused");
    assert_lock_refusal(&probed, "symlink or not a regular file");
    // The target directory and its emptiness stay as they were.
    assert!(std::fs::metadata(&target).expect("target").is_dir());
    assert!(
        std::fs::read_dir(&target)
            .expect("target walk")
            .next()
            .is_none(),
        "the dir was disturbed"
    );
}

/// A real FIFO at the lock path (SPEC-w10 G0a's case, per QA's G0-qa
/// note): refused by `acquire` and by `lock_state` with no hang. The
/// test's own watchdog fails at two seconds, so a hang is a failing
/// test rather than an idle seat. The FIFO is planted by the
/// `mkfifo` CLI in a temp dir, the way the W9 tests plant their git
/// roots; without the tool the case skips, never fakes.
#[test]
fn a_real_fifo_at_the_lock_path_is_refused_without_a_hang() {
    let (data, lock) = bare_repo("rolocfifo");
    let planted = Command::new("mkfifo")
        .arg(&lock)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !planted {
        eprintln!("skipped: no mkfifo on this runner");
        return;
    }
    assert!(
        !std::fs::symlink_metadata(&lock)
            .expect("fifo stays")
            .is_file(),
        "mkfifo planted something else"
    );
    let db = data.db();

    // Both checks run on a bounded pane; a return past the timeout is
    // the failure, and neither path may touch the FIFO.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let refused = WriterLock::acquire(&db).expect_err("refused acquire");
            let _ = tx.send(refused);
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the FIFO acquire hung");
        assert_lock_refusal(&refused, "symlink or not a regular file");
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let refused = lock_state(&db).expect_err("refused state");
            let _ = tx.send(refused);
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the FIFO probe hung");
        assert_lock_refusal(&refused, "symlink or not a regular file");
    });
    // The refusal never disturbed the FIFO: still a non-file, and
    // nothing new appeared beside it.
    assert!(
        !std::fs::symlink_metadata(&lock)
            .expect("fifo stays")
            .is_file()
    );
    let _ = fs::remove_file(&lock);
}

/// The W11 G0a race: the FIFO lands between the pre-open shape check
/// and the open. The hook in `open_lock_file` fires at exactly that
/// seam, plants the FIFO, and clears itself; the refused shape still
/// answers within the W10 bound, and the FIFO's `O_NONBLOCK` open
/// means no thread holds off either side. The named hook is also what
/// QA uses to plant its own between-checks shapes.
#[test]
fn a_fifo_racing_the_open_is_refused_without_a_hang() {
    let (data, lock) = bare_repo("rolockrace");
    let db = data.db();
    let lock_for_hook = lock.clone();
    let raced = std::sync::atomic::AtomicBool::new(false);
    let raced_hook = std::sync::Arc::new(raced);
    let raced_once = raced_hook.clone();
    let hook: Box<dyn Fn(&std::path::Path) + Send> = Box::new(move |_| {
        if !raced_once.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let _ = Command::new("mkfifo")
                .arg(&lock_for_hook)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    });
    crate::lock::race_lock_window(Some(hook));
    let refused = std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let _ = tx.send(WriterLock::acquire(&db).err());
        });
        rx.recv_timeout(Duration::from_secs(2))
    });
    crate::lock::race_lock_window(None);
    let refused = refused
        .expect("the race test hung past two seconds")
        .expect("refused");
    assert_lock_refusal(&refused, "symlink or not a regular file");
    // The refused shape never became a lock file by the other side.
    assert!(
        !std::fs::symlink_metadata(&lock)
            .expect("fifo stays")
            .is_file()
    );
    let _ = fs::remove_file(&lock);
}

/// The W11 G0 fix's second shape: the hook plants a SYMLINK between
/// the pre-open shape check and the open. On the flagged targets the
/// open fails with `ELOOP` — mapped to the store lock-file refusal,
/// never a raw `Io` error — and on the others the shape re-check
/// refuses the same way. Either way the caller sees the store's own
/// refusal, never an absolute path and never a follow.
#[test]
fn a_symlink_racing_the_open_is_refused_as_a_lock_file() {
    use crate::StoreError;

    let (data, lock) = bare_repo("rolocelink");
    let db_path = data.db();
    let raced = std::sync::atomic::AtomicBool::new(false);
    let raced_once = std::sync::Arc::new(raced);
    let raced_hook = raced_once.clone();
    let hook_target = lock.clone();
    let repo = data.repo_dir();
    let hook: Box<dyn Fn(&std::path::Path) + Send> = Box::new(move |_| {
        if !raced_hook.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let target = repo.join("target.rs");
            let _ = std::fs::write(&target, b"raced");
            let _ = std::os::unix::fs::symlink(&target, &hook_target);
        }
    });
    crate::lock::race_lock_window(Some(hook));
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let _ = tx.send(WriterLock::acquire(&db_path).err());
        });
        let refused = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the symlink race hung past two seconds")
            .expect("refused");
        assert!(
            matches!(refused, StoreError::DataDirectory(..)),
            "{refused:?}"
        );
    });
    crate::lock::race_lock_window(None);
    // The refused link left its target untouched.
    assert!(
        std::fs::read_to_string(data.repo_dir().join("target.rs")).is_ok(),
        "the target was disturbed"
    );
    let _ = std::fs::remove_file(&lock);
    let _ = std::fs::remove_file(data.repo_dir().join("target.rs"));
}
