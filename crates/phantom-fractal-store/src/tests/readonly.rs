//! The strictly read-only open (W9 G3): the getters serve, nothing in
//! the data dir moves — no `-wal`, no `-shm`, no journal, no chmod, no
//! migration — and a write attempt is refused.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use phantom_fractal_lang_rust::RustLanguage;

use super::{TempData, assert_outside_workspace};
use crate::{Store, StoreError};

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

/// Entries of `root`, sorted, as `open_read_only`'s tests compare them.
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
/// opens later.
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

/// No mode moves; the database file -- bytes, mode, and mtime -- and
/// every non-sidecar file stay put; no entry disappears.
///
/// One SQLite-contract caveat is written here so the tests say what
/// they mean: SQLite serves a read-only WAL database by creating its
/// own `-wal`/`-shm` sidecars during the reader's life, and which of
/// them linger after the read closes is the SQLite build's choice;
/// those sidecars are the read protocol, not the store's writes. The
/// rollback `-journal` is the one sidecar shape the read-only open
/// never takes, and `graph.db` never moves at all.
fn assert_stable(before: &[Entry], after: &[Entry], what: &str) {
    fn sidecar(name: &str) -> bool {
        name.ends_with("-shm") || name.ends_with("-wal")
    }
    for a in after {
        if sidecar(&a.relative) && !before.iter().any(|b| b.relative == a.relative) {
            continue;
        }
        if !before.iter().any(|b| b.relative == a.relative) {
            panic!("{what}: {} appeared", a.relative);
        }
    }
    for b in before {
        if !after.iter().any(|a| a.relative == b.relative) {
            panic!("{what}: {} disappeared", b.relative);
        }
    }
    for b in before {
        let a = &after
            .iter()
            .find(|a| a.relative == b.relative)
            .expect("matched");
        assert_eq!(b.mode, a.mode, "{what}: {} mode moved", b.relative);
        if b.is_dir {
            // The index dir's mtime moves with the reader's own
            // sidecars coming and going.
            continue;
        }
        if !b.relative.ends_with("-shm") {
            assert_eq!(b.mtime, a.mtime, "{what}: {} mtime moved", b.relative);
            assert_eq!(b.digest, a.digest, "{what}: {} bytes moved", b.relative);
        }
    }
}

/// A read that touches no SQLite at all must move nothing at all: the
/// strict form the schema-newer preflight takes.
fn assert_frozen(before: &[Entry], after: &[Entry], what: &str) {
    assert_eq!(before, after, "{what}: the data dir moved");
}

#[test]
fn open_read_only_serves_the_getters_on_a_closed_database() {
    let data = written_store("rooclosed");
    let before = snapshot(data.data());
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert_serves(&store);
    drop(store);
    assert_stable(&before, &snapshot(data.data()), "the data dir");
    // A rollback `-journal` would be a write path this open never
    // takes; the reader's own `-wal`/`-shm` by-products are SQLite's.
    let left = snapshot(data.data());
    let journals: Vec<&str> = left
        .iter()
        .filter(|entry| entry.relative.ends_with("-journal"))
        .map(|entry| entry.relative.as_str())
        .collect();
    assert!(journals.is_empty(), "{journals:?} was left behind");
}

#[test]
fn open_read_only_refuses_writes() {
    let data = written_store("rowrites");
    let before = snapshot(data.data());
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
    drop(store);
    drop(language);
    assert_stable(&before, &snapshot(data.data()), "refused write");
}

#[test]
fn open_read_only_reads_a_live_wal_database_without_a_move() {
    let data = written_store("rowal");
    // The writer holds its connection open, so `-wal` and `-shm` are
    // on disk the whole test.
    let mut writer = Store::open(&data.db()).expect("open writer");
    writer
        .set_root("ledger18")
        .expect("a second write keeps the wal alive");
    let before = snapshot(data.data());
    let present = before
        .iter()
        .filter(|entry| entry.relative.ends_with("-wal") || entry.relative.ends_with("-shm"))
        .count();
    assert_eq!(present, 2, "{before:?}");
    let reader = Store::open_read_only(&data.db()).expect("read while a writer lives");
    assert_eq!(reader.root().expect("root").as_deref(), Some("ledger18"));
    drop(reader);
    let after = snapshot(data.data());
    assert_stable(&before, &after, "live-wal");
    drop(writer);
}

#[test]
fn open_read_only_does_not_chmod_a_0755_index_dir() {
    let data = written_store("ro0755");
    let repo = data.repo_dir();
    let mut perms = fs::metadata(&repo).expect("repo").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&repo, perms).expect("chmod 0755");
    let before = snapshot(data.data());
    let store = Store::open_read_only(&data.db()).expect("read-only open");
    assert_serves(&store);
    drop(store);
    let after = snapshot(data.data());
    assert_stable(&before, &after, "the 0755 index dir");
    let db_entry = after
        .iter()
        .find(|entry| entry.relative.ends_with("/graph.db"))
        .expect("db entry");
    // The read-only open never chmods: what was widened stays widened,
    // and what was set by the writer stays set.
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
    assert_frozen(&before, &snapshot(data.data()), "schema-newer open");
    // A missing database answers without a file: it is a read-only
    // unavailability, not a create.
    let missing = TempData::new("romissing");
    assert!(
        Store::open_read_only(&missing.db())
            .err()
            .map(|err| matches!(err, StoreError::ReadOnlyUnavailable))
            .unwrap_or(false),
        "a missing file must read as read-only unavailability",
    );
    assert!(!missing.db().exists(), "the missing open created a file");
}

#[test]
fn open_read_only_refuses_an_unwritable_index_dir() {
    // SQLite has to serve a WAL read through a `-shm`; when it cannot
    // create one — the index dir takes the write away — the read-only
    // open answers the refusal instead of creating anything.
    let data = written_store("roclosedir");
    let repo = data.repo_dir();
    let mut perms = fs::metadata(&repo).expect("repo").permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&repo, perms).expect("chmod 0555");
    let before = snapshot(data.data());
    let refused = Store::open_read_only(&data.db())
        .err()
        .map(|e| matches!(e, StoreError::ReadOnlyUnavailable));
    // restore, so the temp data can be dropped cleanly
    let mut perms = fs::metadata(&repo).expect("repo").permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&repo, perms).expect("chmod 0700");
    assert!(refused.unwrap_or(false), "an unwritable dir opened");
    // Only entries that existed before the read are compared: the dir
    // mode itself is the test's own chmod.
    let after: Vec<Entry> = snapshot(data.data());
    for a in &after {
        if a.is_dir && a.relative.matches('/').count() >= 1 {
            continue; // the index dir's mode is the test's own chmod
        }
        let Some(b) = before.iter().find(|b| b.relative == a.relative) else {
            panic!("the unwritable refusal: {} appeared", a.relative);
        };
        assert_eq!(b.mode, a.mode, "{} mode moved", b.relative);
        if !a.is_dir && !a.relative.ends_with("-shm") {
            assert_eq!(b.mtime, a.mtime, "{} mtime moved", b.relative);
            assert_eq!(b.digest, a.digest, "{} bytes moved", b.relative);
        }
    }
}
