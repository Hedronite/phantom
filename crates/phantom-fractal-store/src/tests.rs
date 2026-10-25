use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;
use rusqlite::Connection;

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
