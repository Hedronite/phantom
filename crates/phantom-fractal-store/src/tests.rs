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

/// One throwaway data root with a `repos/<name>-<hash>` layout under it.
struct TempData {
    root: PathBuf,
}

impl TempData {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "phantom-fractal-data-{label}-{}-{nanos}",
            process::id()
        ));
        Self { root }
    }

    fn repo_dir(&self) -> PathBuf {
        self.root
            .join("repos")
            .join(format!("ledger-{:08x}", process::id()))
    }

    fn db(&self) -> PathBuf {
        self.repo_dir().join("graph.db")
    }
}

impl Drop for TempData {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
fn mkdir_explicit(path: &Path, mode: u32) {
    use std::os::unix::fs::DirBuilderExt;
    let mut dirs = std::fs::DirBuilder::new();
    dirs.mode(mode);
    dirs.create(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
}

#[test]
#[cfg(unix)]
fn fresh_data_directories_are_created_0700() {
    let data = TempData::new("dirs");
    let db = data.db();
    let store = Store::open(&db).expect("open");
    assert_eq!(store.producer().expect("stamp").as_deref(), Some(PRODUCER));
    for dir in [
        data.root.as_path(),
        data.root.join("repos").as_path(),
        data.repo_dir().as_path(),
    ] {
        assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode_of(&db), 0o600, "graph.db");
}

#[test]
#[cfg(unix)]
fn open_tightens_repos_directories_and_leaves_the_data_root() {
    let data = TempData::new("tighten");
    mkdir_explicit(&data.root, 0o751);
    let repos = data.root.join("repos");
    mkdir_explicit(&repos, 0o755);
    let repo_dir = repos.join("ledger-cafe1111");
    mkdir_explicit(&repo_dir, 0o757);
    let db = repo_dir.join("graph.db");
    Store::open(&db).expect("first open creates the db");
    // A stray user chmod on the repo directory is narrowed on the next
    // open of an existing database.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&repo_dir, std::fs::Permissions::from_mode(0o757)).expect("widen");
    }
    Store::open(&db).expect("second open");
    assert_eq!(mode_of(&data.root), 0o751, "data root");
    assert_eq!(mode_of(&repos), 0o700, "repos");
    assert_eq!(mode_of(&repo_dir), 0o700, "repo dir");
}

#[test]
#[cfg(unix)]
fn open_leaves_a_wider_non_repos_directory_alone() {
    let db = TempDb::new("plain");
    assert_outside_workspace(db.path());
    // The temp dir already exists with a wider umask-carried mode; opening
    // a plain database under it must not change that parent at all.
    let before = mode_of(db.path().parent().expect("parent"));
    Store::open(db.path()).expect("open");
    assert_eq!(
        mode_of(db.path().parent().expect("parent")),
        before,
        "parent mode changed"
    );
}

#[test]
#[cfg(unix)]
fn database_and_wal_files_are_0600_after_index() {
    let data = TempData::new("wal");
    let db = data.db();
    let mut store = Store::open(&db).expect("open");
    let language = RustLanguage::new().expect("grammar");
    let (source, mtime_ns) = fixture();
    store
        .put_source("src/lib.rs", &source, mtime_ns, &language)
        .expect("put");
    let text = db.to_string_lossy().into_owned();
    for suffix in ["", "-wal", "-shm"] {
        let path = format!("{text}{suffix}");
        assert_eq!(mode_of(Path::new(&path)), 0o600, "{path}");
    }
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
    for later in ["meta_table_exists", "check_stamp", "migrate_v1_to_v2"] {
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
        .put_source("src/lib.rs", "pub fn open_ledger() {}\\n", 0, &language)
        .expect("put");
    assert!(store.match_count("open_ledger").expect("match") >= 1);
}

#[test]
fn existing_open_ledger_matches_after_version_1_open() {
    assert_eq!(SCHEMA_VERSION, 2);
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
    assert_eq!(store.schema_version().expect("schema"), 2);
    assert!(store.match_count("open_ledger").expect("match") >= 1);
}

#[test]
fn schema_three_fails_to_open() {
    let db = TempDb::new("v3");
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta(key, value) VALUES ('producer', 'phantom-fractal');
         INSERT INTO meta(key, value) VALUES ('schema_version', '3');
         PRAGMA user_version = 3;",
    )
    .expect("stamp");
    drop(conn);
    let opened = Store::open(db.path());
    assert!(matches!(opened, Err(StoreError::SchemaNewer { found: 3 })));
}
