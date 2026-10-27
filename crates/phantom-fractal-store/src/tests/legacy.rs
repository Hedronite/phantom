//! The W1/W2 store rows that W3 must not regress, updated where the
//! schema stamp moved to 3.

use std::fs;
use std::time::SystemTime;

use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;
use rusqlite::{Connection, OpenFlags};

use super::{
    TempDb, assert_outside_workspace, assert_unchanged, file_bytes, fixture, list_worktree,
    master_rows,
};
use crate::{PRODUCER, SCHEMA_VERSION, Store, StoreError};

#[test]
fn fresh_database_is_stamped_and_other_tables_are_empty() {
    let db = TempDb::new("schema");
    assert_outside_workspace(db.path());
    let store = Store::open(db.path()).expect("open");
    assert_eq!(
        store.producer().expect("producer").as_deref(),
        Some(PRODUCER)
    );
    assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
    assert_eq!(store.user_version().expect("user_version"), SCHEMA_VERSION);
    assert_eq!(store.row_count("files").expect("files"), 0);
    assert_eq!(store.row_count("symbols").expect("symbols"), 0);
    assert_eq!(store.row_count("symbols_fts").expect("fts"), 0);
    // The canon tables W3 leaves empty on a fresh database.
    for table in ["cochange", "file_history", "git_scan", "rank", "index_runs"] {
        assert_eq!(store.row_count(table).expect(table), 0, "{table}");
    }
}

#[test]
fn fixture_extract_round_trips_outside_the_worktree() {
    let before = list_worktree();
    let db = TempDb::new("round");
    assert_outside_workspace(db.path());
    let (source, mtime_ns) = fixture();
    let language = RustLanguage::new().expect("grammar");
    let mut store = Store::open(db.path()).expect("open");
    store
        .put_source("fixtures/extract/items.rs", &source, mtime_ns, &language)
        .expect("put");
    let extract = language.extract(&source).expect("extract");
    assert!(!extract.items.is_empty());
    let stored = store.read_items("fixtures/extract/items.rs").expect("read");
    assert_eq!(stored, extract.items);
    assert_eq!(store.row_count("files").expect("files"), 1);
    // Symbols now carry methods as kind `method` rows too.
    assert_eq!(
        store.row_count("symbols").expect("symbols"),
        i64::try_from(extract.items.len() + extract.methods.len()).expect("len")
    );
    // The canon tables W3 leaves empty on a fresh database.
    for table in ["cochange", "file_history", "git_scan", "rank", "index_runs"] {
        assert_eq!(store.row_count(table).expect(table), 0, "{table}");
    }
    assert_eq!(
        store.file_paths().expect("paths"),
        vec!["fixtures/extract/items.rs".to_owned()]
    );
    drop(store);
    assert_eq!(list_worktree(), before);
}

#[test]
fn database_without_producer_fails_to_open() {
    let db = TempDb::new("stamp");
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;")
        .expect("meta");
    drop(conn);
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::MissingProducer)));
}

#[test]
fn newer_schema_version_fails_to_open() {
    let db = TempDb::new("vnew");
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta(key, value) VALUES ('producer', 'phantom-fractal');
         INSERT INTO meta(key, value) VALUES ('schema_version', '99');
         PRAGMA user_version = 99;",
    )
    .expect("stamp");
    drop(conn);
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::SchemaNewer { found: 99 })));
}

#[test]
fn dataless_sqlite_file_is_not_modified() {
    let db = TempDb::new("dataless");
    assert_outside_workspace(db.path());
    Connection::open(db.path()).expect("empty sqlite");
    let before = file_bytes(db.path());
    let master = master_rows(db.path());
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::ForeignDatabase)));
    assert_unchanged(db.path(), &before);
    assert_eq!(master_rows(db.path()), master);
}

#[test]
fn foreign_producer_is_not_modified() {
    let db = TempDb::new("foreign");
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta(key, value) VALUES ('producer', 'other');
         INSERT INTO meta(key, value) VALUES ('schema_version', '1');",
    )
    .expect("stamp");
    drop(conn);
    let before = file_bytes(db.path());
    let master = master_rows(db.path());
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::ForeignProducer)));
    assert_unchanged(db.path(), &before);
    assert_eq!(master_rows(db.path()), master);
}

#[test]
fn new_file_mode_and_defensive_pragmas() {
    let db = TempDb::new("mode");
    assert_outside_workspace(db.path());
    let store = Store::open(db.path()).expect("open");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(db.path()).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(store.trusted_schema_is_off().expect("trusted"));
    assert!(store.synchronous_is_normal().expect("sync"));
    assert!(store.defensive_is_on().expect("defensive"));
}

#[test]
fn open_flags_omit_uri() {
    let created = crate::open_flags(true);
    let existing = crate::open_flags(false);
    assert!(!created.contains(OpenFlags::SQLITE_OPEN_URI));
    assert!(!existing.contains(OpenFlags::SQLITE_OPEN_URI));
    assert!(created.contains(OpenFlags::SQLITE_OPEN_CREATE));
    assert!(!existing.contains(OpenFlags::SQLITE_OPEN_CREATE));
}

#[test]
fn index_database_path_is_outside_the_repo() {
    let repo = std::env::temp_dir().join(format!(
        "phantom-index-repo-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&repo).expect("repo");
    let db = crate::index_database_path(&repo);
    let repo = repo.canonicalize().expect("repo path");
    assert!(db.ends_with("graph.db"));
    assert!(!db.starts_with(&repo), "{db:?} is inside {repo:?}");
    let _ = fs::remove_dir_all(&repo);
}

#[test]
fn harden_is_first_on_an_existing_database() {
    let db = TempDb::new("harden-order");
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "DROP TRIGGER symbols_ai;
             DROP TRIGGER symbols_ad;
             DROP TRIGGER symbols_au;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';
             PRAGMA user_version = 1;",
        )
        .expect("downgrade");
    }
    crate::open_trace::clear();
    let store = Store::open(db.path()).expect("open existing");
    drop(store);
    let steps = crate::open_trace::steps();
    assert!(!steps.is_empty(), "no open steps recorded");
    assert_eq!(steps.first().copied(), Some("harden"), "{steps:?}");
    let harden_at = steps
        .iter()
        .position(|step| *step == "harden")
        .expect("harden recorded");
    for later in [
        "meta_table_exists",
        "check_stamp",
        "migrate_v1_to_v2",
        "migrate_v2_to_v3",
    ] {
        let at = steps
            .iter()
            .position(|step| *step == later)
            .unwrap_or_else(|| panic!("{later} not recorded: {steps:?}"));
        assert!(at > harden_at, "{later} at {at}, {steps:?}");
    }
}

#[test]
fn disallowed_table_name_is_not_unreadable_stamp() {
    let db = TempDb::new("table");
    let store = Store::open(db.path()).expect("open");
    let err = store.row_count("not_a_table").expect_err("rejected");
    assert!(matches!(err, StoreError::UnknownTable));
    assert!(!matches!(err, StoreError::UnreadableStamp));
}

#[test]
fn stored_blake3_matches_put_source_and_missing_is_none() {
    let db = TempDb::new("blake");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let source = "fn open_ledger() {}\n";
    let path = "src/lib.rs";
    let language = RustLanguage::new().expect("grammar");
    store.put_source(path, source, 0, &language).expect("put");
    let expected = blake3::hash(source.as_bytes());
    let stored = store.stored_blake3(path).expect("hash");
    assert_eq!(stored.as_deref(), Some(expected.as_bytes().as_slice()));
    assert!(
        store
            .stored_blake3("missing.rs")
            .expect("missing")
            .is_none()
    );
}

#[test]
fn aliased_use_is_stored_and_items_remain() {
    let db = TempDb::new("refs");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let (source, mtime_ns) = super::fixture_for("imports.rs");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("fixtures/extract/imports.rs", &source, mtime_ns, &language)
        .expect("put");
    let language2 = RustLanguage::new().expect("grammar");
    let extract = language2.extract(&source).expect("extract");
    let aliased = extract
        .imports
        .iter()
        .find(|import| import.alias.as_deref() == Some("IoResult"))
        .expect("aliased use");
    let texts = store
        .ref_texts("fixtures/extract/imports.rs")
        .expect("refs");
    assert!(texts.iter().any(|text| text == &aliased.path));
    assert_eq!(texts.len(), extract.imports.len());
    assert_eq!(
        store
            .read_items("fixtures/extract/imports.rs")
            .expect("items"),
        extract.items
    );
}

#[test]
fn open_ledger_match_is_bound_and_a_quote_cannot_change_sql() {
    let db = TempDb::new("match");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let (source, mtime_ns) = fixture();
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("fixtures/extract/items.rs", &source, mtime_ns, &language)
        .expect("put");
    assert!(store.match_count("open_ledger").expect("written") >= 1);
    assert_eq!(store.match_count("not_written_zz").expect("absent"), 0);
    let tricky = "open_ledger\" OR name:open_ledger";
    assert!(!crate::MATCH_SQL.contains(tricky));
    assert!(crate::MATCH_SQL.contains("?1"));
    assert!(!crate::MATCH_SQL.contains("format!"));
    assert_eq!(crate::phrase_quote("a\"b"), "\"a\"\"b\"");
    assert_eq!(store.match_count(tricky).expect("quoted"), 0);
}

#[test]
fn version_1_database_indexes_open_ledger_on_open() {
    let db = TempDb::new("v1");
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "DROP TRIGGER symbols_ai;
             DROP TRIGGER symbols_ad;
             DROP TRIGGER symbols_au;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';
             PRAGMA user_version = 1;",
        )
        .expect("downgrade");
    }
    let mut store = Store::open(db.path()).expect("migrate");
    assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
    assert_eq!(store.user_version().expect("user_version"), SCHEMA_VERSION);
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("src/lib.rs", "pub fn open_ledger() {}\n", 0, &language)
        .expect("put");
    assert!(store.match_count("open_ledger").expect("match") >= 1);
}

#[test]
fn existing_open_ledger_matches_after_version_1_open() {
    assert_eq!(SCHEMA_VERSION, 3);
    let db = TempDb::new("pre");
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "DROP TRIGGER symbols_ai;
             DROP TRIGGER symbols_ad;
             DROP TRIGGER symbols_au;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';
             PRAGMA user_version = 1;
             INSERT INTO files(path, lang, blake3, size, mtime_ns, generated, parse_status)
             VALUES ('src/lib.rs', 'rust', X'00', 1, 0, 0, 'ok');
             INSERT INTO symbols(
               file_id, kind, name, qual_name, stable_key, visibility, start_line
             ) VALUES (
               1, 'function', 'open_ledger', 'open_ledger',
               'src/lib.rs#open_ledger#function#0', 'public', 1
             );",
        )
        .expect("seed");
    }
    let store = Store::open(db.path()).expect("migrate");
    assert_eq!(store.schema_version().expect("schema"), 3);
    assert!(store.match_count("open_ledger").expect("match") >= 1);
}

#[test]
fn version_2_database_migrates_to_3() {
    assert_eq!(SCHEMA_VERSION, 3);
    let db = TempDb::new("v2");
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "ALTER TABLE symbols DROP COLUMN scope;
             ALTER TABLE symbols DROP COLUMN owner;
             ALTER TABLE symbols DROP COLUMN owner_path;
             ALTER TABLE symbols DROP COLUMN trait_name;
             ALTER TABLE modules DROP COLUMN scope;
             ALTER TABLE refs DROP COLUMN scope;
             ALTER TABLE refs DROP COLUMN receiver;
             ALTER TABLE refs DROP COLUMN shadowed;
             ALTER TABLE refs DROP COLUMN alias;
             ALTER TABLE refs DROP COLUMN glob;
             UPDATE meta SET value = '2' WHERE key = 'schema_version';
             PRAGMA user_version = 2;",
        )
        .expect("downgrade");
    }
    let mut store = Store::open(db.path()).expect("migrate");
    assert_eq!(store.schema_version().expect("schema"), 3);
    assert_eq!(store.user_version().expect("user_version"), 3);
    let language = RustLanguage::new().expect("grammar");
    let (source, mtime_ns) = fixture();
    store
        .put_source("fixtures/extract/items.rs", &source, mtime_ns, &language)
        .expect("put");
    assert!(store.match_count("open_ledger").expect("match") >= 1);
    assert_eq!(
        store
            .read_items("fixtures/extract/items.rs")
            .expect("items"),
        RustLanguage::new()
            .expect("grammar")
            .extract(&source)
            .expect("extract")
            .items
    );
}

#[test]
fn schema_four_fails_to_open() {
    let db = TempDb::new("v4");
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta(key, value) VALUES ('producer', 'phantom-fractal');
         INSERT INTO meta(key, value) VALUES ('schema_version', '4');
         PRAGMA user_version = 4;",
    )
    .expect("stamp");
    drop(conn);
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::SchemaNewer { found: 4 })));
}
