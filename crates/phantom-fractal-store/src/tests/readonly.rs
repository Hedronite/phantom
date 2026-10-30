//! The guarded read-only open (W9 G3, PM ruling): the gate refuses a
//! busy database before any open, the `mode=ro&immutable=1` read moves
//! nothing in the data dir, the getters serve, writes refuse, and the
//! post-read re-check mirrors the gate.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use phantom_fractal_lang_rust::RustLanguage;

use super::{TempData, assert_outside_workspace};
use crate::{LockState, Store, StoreError, WriterLock, lock_state};

/// One entry of a data dir walk: the relative path, the kind, the
/// mode, the mtime, and — for files — the content hash.
#[derive(Debug, PartialEq, Eq, Clone)]
struct Entry {
    relative: String,
    is_dir: bool,
    mode: u32,
    mtime: (i64, i64),
    digest: String,
}

/// Entries of `root`, sorted: every entry's bytes, modes, and mtimes,
/// directories included, as the guarded read's tests compare them.
fn snapshot(root: &Path) -> Vec<Entry> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) {
        let entries = fs::read_dir(dir).expect("data walk");
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("inside the root")
                .to_string_lossy()
                .into_owned();
            let meta = fs::metadata(&path).expect("entry");
            let mtime = (meta.mtime(), meta.mtime_nsec());
            let (is_dir, digest) = if meta.is_dir() {
                walk(root, &path, out);
                (true, String::new())
            } else {
                (
                    false,
                    blake3::hash(&fs::read(&path).expect("read"))
                        .to_hex()
                        .to_string(),
                )
            };
            out.push(Entry {
                relative,
                is_dir,
                mode: meta.permissions().mode() & 0o7777,
                mtime,
                digest,
            });
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_unstable_by(|a, b| a.relative.cmp(&b.relative));
    out
}

/// A store with written state on a git-shaped root, closed so the read
/// opens later and the data dir carries no sidecar of its own.
fn written_store(label: &str) -> TempData {
    let data = TempData::new(label);
    let language = RustLanguage::new().expect("grammar");
    // The open creates the chain, so the outside-workspace check runs
    // after it, like the other TempData tests.
    let mut store = Store::open(&data.db()).expect("open writer");
    assert_outside_workspace(&data.db());
    store.set_root("ledger17").expect("root");
    store.set_index_head(Some("aaaa9999")).expect("head");
    store
        .put_source(
            "src/lib.rs",
            "pub fn ping() -> u8 {\n    1\n}\n",
            10,
            &language,
        )
        .expect("put");
    store.write_rank(4, &[], &[(1, 1.0)]).expect("rank");
    drop(store);
    drop(language);
    data
}

fn assert_serves(store: &Store) {
    assert_eq!(
        store.root().expect("root").as_deref(),
        Some("ledger17"),
        "root reads"
    );
    assert_eq!(
        store.index_head().expect("head").as_deref(),
        Some("aaaa9999"),
        "index_head reads"
    );
    assert_eq!(store.row_count("files").expect("count"), 1);
    let stats = store.file_stats().expect("stats");
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].path, "src/lib.rs");
    assert_eq!(store.workspace_fingerprint().expect("fingerprint"), None);
    assert_eq!(store.rank_epoch().expect("rank epoch"), 4);
    assert_eq!(store.graph_epoch().expect("graph epoch"), 0);
}

#[test]
fn open_read_only_serves_the_getters_and_moves_nothing() {
    let data = written_store("rooclosed");
    let before = snapshot(data.data());
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert_serves(&store);
    assert!(store.read_only_still_valid().expect("valid"));
    drop(store);
    assert_eq!(
        snapshot(data.data()),
        before,
        "the guarded read moved the data dir"
    );
    // The refused writes also refused without moving anything.
    let language = RustLanguage::new().expect("grammar");
    let mut store = Store::open_read_only(&data.db()).expect("read-only open");
    assert!(
        store.set_root("changed").is_err(),
        "a read-only store wrote a root"
    );
    assert!(
        store
            .put_source("src/x.rs", "pub fn x() {}\n", 0, &language)
            .is_err(),
        "a read-only store wrote a source"
    );
    assert!(store.read_only_still_valid().expect("still valid"));
    drop(store);
    drop(language);
    assert_eq!(
        snapshot(data.data()),
        before,
        "a refused write moved the data dir"
    );
}

#[test]
fn open_read_only_gates_on_sidecars_and_a_held_lock() {
    // A writer connection alive keeps `-wal` and `-shm` on disk; the
    // guarded read refuses them before any SQLite work.
    let data = written_store("rowal");
    let mut writer = Store::open(&data.db()).expect("open writer");
    writer
        .set_root("ledger18")
        .expect("a write keeps the sidecars alive");
    let before = snapshot(data.data());
    let sidecars: Vec<&str> = before
        .iter()
        .filter(|entry| entry.relative.ends_with("-wal") || entry.relative.ends_with("-shm"))
        .map(|entry| entry.relative.as_str())
        .collect();
    assert_eq!(sidecars.len(), 2, "{before:?}");
    match Store::open_read_only(&data.db()) {
        Err(StoreError::ReadOnlyUnavailable) => {}
        Ok(_) => panic!("a live-wal database opened read-only"),
        Err(other) => panic!("{other:?}"),
    }
    drop(writer);
    // A clean close leaves the data dir as the writer found it, bar
    // the write's own work: the gate closes and the read serves.
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert_eq!(store.root().expect("root").as_deref(), Some("ledger18"));
    drop(store);

    // The held writer lock gates the same way, with no sidecar at all.
    let data = TempData::new("roheld");
    std::fs::create_dir_all(data.db().parent().expect("parent")).expect("repo dir");
    let _guard = WriterLock::acquire(&data.db()).expect("acquire");
    assert!(matches!(lock_state(&data.db()), Ok(LockState::Held)));
    match Store::open_read_only(&data.db()) {
        Err(StoreError::ReadOnlyUnavailable) => {}
        Ok(_) => panic!("a locked database opened read-only"),
        Err(other) => panic!("{other:?}"),
    }
}

#[test]
fn read_only_still_valid_mirrors_the_gate() {
    let data = written_store("rostale");
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert!(store.read_only_still_valid().expect("valid"));
    // A writer arriving after the reads makes the reads stale.
    let writer_lock = WriterLock::acquire(&data.db()).expect("acquire");
    assert!(!store.read_only_still_valid().expect("stale"));
    drop(writer_lock);
    assert!(store.read_only_still_valid().expect("valid again"));
}

#[test]
fn open_read_only_needs_no_writable_index_dir_and_never_chmods() {
    let data = written_store("ro0755");
    {
        // The guarded read never creates a sidecar, so an index dir
        // with the writes taken away still serves — and nothing chmods.
        let language = RustLanguage::new().expect("grammar");
        let mut store = Store::open(&data.db()).expect("open writer");
        store.set_root("ledger51").expect("root");
        drop(store);
        drop(language);
        let repo = data.repo_dir();
        let mut perms = fs::metadata(&repo).expect("repo").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&repo, perms).expect("chmod 0755");
    }
    let before = snapshot(data.data());
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert_eq!(store.root().expect("root").as_deref(), Some("ledger51"));
    drop(store);
    let after = snapshot(data.data());
    assert_eq!(after, before, "the 0755 index dir moved");
    let db_entry = after
        .iter()
        .find(|entry| entry.relative.ends_with("/graph.db"))
        .expect("db entry");
    assert_eq!(db_entry.mode, 0o600);
    let repo_entry = after
        .iter()
        .find(|entry| entry.is_dir && entry.relative.matches('/').count() == 1)
        .expect("repo dir entry");
    assert_eq!(repo_entry.mode, 0o755);
}

#[test]
fn open_read_only_refuses_a_schema_newer_database_without_a_touch() {
    let data = written_store("ronewer");
    {
        // Stamp a newer version through a direct write connection:
        // setup, not the behavior under test.
        let setup = rusqlite::Connection::open(data.db()).expect("setup open");
        setup
            .execute(
                "UPDATE meta SET value = '9' WHERE key = 'schema_version'",
                [],
            )
            .expect("stamp");
        setup.pragma_update(None, "user_version", 9).expect("stamp");
    }
    let before = snapshot(data.data());
    match Store::open_read_only(&data.db()) {
        Err(StoreError::SchemaNewer { found }) => assert_eq!(found, 9),
        Ok(_) => panic!("a schema-newer database must not open"),
        Err(other) => panic!("{other:?}"),
    }
    assert_eq!(
        snapshot(data.data()),
        before,
        "the refused open moved the data dir"
    );
    // A missing database answers without a file: it is a read-only
    // unavailability, not a create.
    let missing = TempData::new("romissing");
    let open_error = Store::open_read_only(&missing.db()).err();
    assert!(
        matches!(open_error, Some(StoreError::ReadOnlyUnavailable)),
        "a missing file must read as read-only unavailability: {open_error:?}",
    );
    assert!(!missing.db().exists(), "the missing open created a file");
    // No error of the guarded open carries a path.
    for message in [
        StoreError::ReadOnlyUnavailable.to_string(),
        StoreError::LockHeld.to_string(),
        StoreError::SchemaNewer { found: 9 }.to_string(),
    ] {
        assert!(!message.contains('/'), "{message} carries a path");
        assert!(!message.contains("graph.db"), "{message} carries a path");
    }
}
