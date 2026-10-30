//! Shared throwaway fixtures for the store's tests, plus the module
//! split: `legacy` holds the W1/W2 rows still under grade, `dirs` the
//! directory work, `writes` the replace/remove/persist rows, `graph`
//! the crate-graph API rows, `lock` the W9 writer lock, `meta` the W9
//! meta setters, and `dump` the canonical dump.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

mod cochange;
mod detail;
mod dirs;
mod dump;
mod graph;
mod legacy;
mod lock;
mod meta;
mod rank;
mod writes;

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

fn file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("read db")
}

fn assert_unchanged(path: &Path, before: &[u8]) {
    assert_eq!(file_bytes(path), before);
    let text = path.to_string_lossy();
    assert!(!Path::new(&format!("{text}-wal")).exists());
    assert!(!Path::new(&format!("{text}-shm")).exists());
}

fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn mkdir_explicit(path: &Path, mode: u32) {
    use std::os::unix::fs::DirBuilderExt;
    let mut dirs = std::fs::DirBuilder::new();
    dirs.mode(mode);
    dirs.create(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
}

fn fixture_for(name: &str) -> (String, i64) {
    let path = workspace_root().join("fixtures").join("extract").join(name);
    let source = fs::read_to_string(&path).expect("fixture");
    let mtime_ns = fs::metadata(&path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|dur| i64::try_from(dur.as_nanos()).ok())
        .unwrap_or(0);
    (source, mtime_ns)
}

fn fixture() -> (String, i64) {
    fixture_for("items.rs")
}

fn master_rows(path: &Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(path).expect("sqlite");
    let mut statement = conn
        .prepare("SELECT type, name FROM sqlite_master ORDER BY 1, 2")
        .expect("master");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}
