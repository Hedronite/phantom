use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;
use rusqlite::{Connection, OpenFlags};

use crate::schema::EMPTY_TABLES;
use crate::{PRODUCER, SCHEMA_VERSION, Store, StoreError};

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-store-{label}-{}-{nanos}.db",
            process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let text = self.path.to_string_lossy().into_owned();
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(format!("{text}-wal"));
        let _ = fs::remove_file(format!("{text}-shm"));
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn assert_outside_workspace(path: &Path) {
    let workspace = workspace_root().canonicalize().expect("workspace");
    let parent = path.parent().expect("parent").canonicalize().expect("temp");
    assert!(
        !parent.starts_with(&workspace),
        "database parent {parent:?} is inside {workspace:?}"
    );
}

fn list_worktree() -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == "target" || name == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    let root = workspace_root().canonicalize().expect("workspace");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    out
}

fn fixture() -> (String, i64) {
    let path = workspace_root().join("fixtures/extract/items.rs");
    let source = fs::read_to_string(&path).expect("fixture");
    let mtime_ns = fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|dur| i64::try_from(dur.as_nanos()).ok())
        .unwrap_or(0);
    (source, mtime_ns)
}

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
    for table in EMPTY_TABLES {
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
    assert!(extract.imports.is_empty());
    let stored = store.read_items("fixtures/extract/items.rs").expect("read");
    assert_eq!(stored, extract.items);
    assert_eq!(store.row_count("files").expect("files"), 1);
    assert_eq!(
        store.row_count("symbols").expect("symbols"),
        i64::try_from(extract.items.len()).expect("len")
    );
    for table in EMPTY_TABLES {
        assert_eq!(store.row_count(table).expect(table), 0, "{table}");
    }
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
    let db = TempDb::new("newer");
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

fn file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read db")
}

fn assert_unchanged(path: &Path, before: &[u8]) {
    assert_eq!(file_bytes(path), before);
    let text = path.to_string_lossy();
    assert!(!Path::new(&format!("{text}-wal")).exists());
    assert!(!Path::new(&format!("{text}-shm")).exists());
}

fn master_rows(path: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(path).expect("sqlite");
    let mut statement = conn
        .prepare("SELECT type, name FROM sqlite_master ORDER BY 1, 2")
        .expect("master");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
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
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
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
    let source =
        fs::read_to_string(workspace_root().join("fixtures/extract/imports.rs")).expect("fixture");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("fixtures/extract/imports.rs", &source, 0, &language)
        .expect("put");
    let extract = language.extract(&source).expect("extract");
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
    let source =
        fs::read_to_string(workspace_root().join("fixtures/extract/items.rs")).expect("fixture");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("fixtures/extract/items.rs", &source, 0, &language)
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
