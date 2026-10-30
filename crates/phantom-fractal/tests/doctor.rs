//! `fract doctor` (SPEC-w9 G3b, D11): seven checks in a fixed order,
//! always exit 0, no path, and no write: not a byte, a mode or an mtime
//! under the data directory changes, and nothing is repaired.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use phantom_fractal_store::WriterLock;
use serde_json::Value;

use support::{Repo, TempDir, copy_fixture, history};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const ORDER: [&str; 7] = [
    "data_dir",
    "databases",
    "locks",
    "extractor",
    "grammar",
    "network",
    "git",
];

/// A file's length, mtime and bytes.
type Content = (u64, std::time::SystemTime, Vec<u8>);

/// Every entry under `data`: path, kind, mode and, for files other than
/// SQLite's shared memory, its [`Content`].
fn state(data: &Path) -> Vec<(PathBuf, bool, u32, Option<Content>)> {
    let mut out = Vec::new();
    let mut stack = vec![data.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            let meta = fs::symlink_metadata(&path).expect("meta");
            let mode = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode()
                }
                #[cfg(not(unix))]
                {
                    0
                }
            };
            let content =
                (!meta.is_dir() && !path.to_string_lossy().ends_with("-shm")).then(|| {
                    (
                        meta.len(),
                        meta.modified().expect("mtime"),
                        fs::read(&path).unwrap_or_default(),
                    )
                });
            if meta.is_dir() {
                stack.push(path.clone());
            }
            out.push((path, meta.is_dir(), mode, content));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn run(bin: &str, data: &Path, dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(bin)
        .args(args)
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
        .output()
        .expect("run cli");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    (
        output.status.code(),
        stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned(),
    )
}

/// `doctor`'s data, checked for exit 0, the order, no `meta`, no path and
/// no write, under both binary names.
fn doctor(data: &Path, dir: &Path) -> Value {
    let before = state(data);
    let existed = data.exists();
    let (code, line) = run(BINS[0], data, dir, &["doctor"]);
    assert_eq!(code, Some(0), "{line}");
    assert_eq!(run(BINS[1], data, dir, &["doctor"]), (code, line.clone()));
    assert_eq!(state(data), before, "doctor wrote");
    assert_eq!(data.exists(), existed, "doctor made the data directory");
    assert!(!line.contains("\"meta\""), "{line}");
    for path in [data, dir] {
        assert!(!line.contains(path.to_str().expect("utf-8")), "{line}");
    }
    assert!(!line.contains("\"/") && !line.contains(" /"), "{line}");
    let data = serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone();
    let names: Vec<&str> = data["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|check| check["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ORDER);
    let all = data["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .all(|check| check["ok"] == true);
    assert_eq!(data["ok"], all);
    data
}

fn check<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == name)
        .expect("check")
}

/// A git root and a fixture copy, both indexed into one data directory.
struct World {
    repo: Repo,
    dir: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            repo: Repo::new("doctor", &history()),
            dir: TempDir::new("doctor-trees"),
            data: TempDir::new("doctor-data"),
        };
        assert_eq!(
            run(
                BINS[0],
                world.data.path(),
                &world.repo.root,
                &["index", "."]
            )
            .0,
            Some(0)
        );
        let graph = world.dir.path().join("graph");
        copy_fixture("graph", &graph);
        assert_eq!(
            run(BINS[0], world.data.path(), &graph, &["index", "."]).0,
            Some(0)
        );
        world
    }

    fn key(&self, prefix: &str) -> String {
        fs::read_dir(self.data.path().join("repos"))
            .expect("repos")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8")
            })
            .find(|key| key.starts_with(prefix))
            .expect("key")
    }

    fn database(&self, prefix: &str) -> PathBuf {
        self.data
            .path()
            .join("repos")
            .join(self.key(prefix))
            .join("graph.db")
    }
}

#[test]
fn a_new_install_with_no_data_directory_is_ok() {
    let dir = TempDir::new("doctor-empty");
    let data = dir.path().join("never-made");
    let data = doctor(&data, dir.path());
    assert_eq!(data["ok"], true);
    assert_eq!(check(&data, "databases")["detail"], "no index yet");
    assert_eq!(
        check(&data, "network")["detail"],
        "none: Phantom has no network code"
    );
    assert_eq!(check(&data, "extractor")["detail"], "7");
    let grammar = check(&data, "grammar")["detail"].as_str().expect("grammar");
    assert!(grammar.starts_with("tree-sitter-rust 0."), "{grammar}");
    // An extra argument is usage.
    let (code, line) = run(
        BINS[0],
        &dir.path().join("x"),
        dir.path(),
        &["doctor", "now"],
    );
    assert_eq!((code, line.as_str()), (Some(1), ""));
}

#[test]
fn a_healthy_data_directory_passes_every_check() {
    let world = World::new();
    let data = doctor(world.data.path(), world.dir.path());
    assert_eq!(data["ok"], true, "{data}");
    let repo = world.key("repo-");
    let graph = world.key("graph-");
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(git.contains(&format!("{repo}: HEAD reads")), "{git}");
    assert!(git.contains(&format!("{graph}: not a git root")), "{git}");
    let databases = check(&data, "databases")["detail"]
        .as_str()
        .expect("db")
        .to_owned();
    assert!(
        databases.contains(&format!("{repo}: schema 3")),
        "{databases}"
    );
    assert_eq!(
        check(&data, "locks")["detail"],
        format!("{graph}: free; {repo}: free")
    );
}

#[cfg(unix)]
#[test]
fn a_0644_database_and_an_open_directory_are_flagged_and_left_as_they_are() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let database = world.database("graph-");
    fs::set_permissions(&database, fs::Permissions::from_mode(0o644)).expect("chmod");
    let data = doctor(world.data.path(), world.dir.path());
    assert_eq!(data["ok"], false);
    let databases = check(&data, "databases");
    assert_eq!(databases["ok"], false);
    assert!(
        databases["detail"]
            .as_str()
            .expect("detail")
            .contains("graph.db is 0644, not 0600"),
        "{databases}"
    );
    assert_eq!(check(&data, "data_dir")["ok"], true);
    // Still 0644: nothing was repaired.
    let mode = fs::metadata(&database).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
    fs::set_permissions(&database, fs::Permissions::from_mode(0o600)).expect("chmod back");

    let dir = database.parent().expect("dir").to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod dir");
    let data = doctor(world.data.path(), world.dir.path());
    assert_eq!(check(&data, "data_dir")["ok"], false);
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(
        git.contains("not checked, its directories are not private"),
        "{git}"
    );
    let mode = fs::metadata(&dir).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "doctor narrowed the directory");
}

#[test]
fn a_schema_newer_database_is_flagged_and_never_opened() {
    let world = World::new();
    let database = world.database("graph-");
    // `user_version`, at byte 60 of the header, as a newer binary would
    // stamp it; the index was closed, so there is no WAL to outrank it.
    let mut bytes = fs::read(&database).expect("read");
    assert!(
        !database.with_file_name("graph.db-wal").exists()
            || fs::metadata(database.with_file_name("graph.db-wal"))
                .expect("wal")
                .len()
                == 0
    );
    bytes[60..64].copy_from_slice(&99_u32.to_be_bytes());
    fs::write(&database, &bytes).expect("write");
    let data = doctor(world.data.path(), world.dir.path());
    assert_eq!(data["ok"], false);
    let databases = check(&data, "databases");
    assert_eq!(databases["ok"], false);
    assert!(
        databases["detail"]
            .as_str()
            .expect("detail")
            .contains("schema 99, newer than this binary's 3"),
        "{databases}"
    );
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(
        git.contains("not checked, its schema is not this binary's"),
        "{git}"
    );
    assert_eq!(fs::read(&database).expect("read"), bytes, "untouched");
}

#[test]
fn a_held_lock_is_reported_and_is_no_fault() {
    let world = World::new();
    let guard = WriterLock::acquire(&world.database("repo-")).expect("lock");
    let data = doctor(world.data.path(), world.dir.path());
    let locks = check(&data, "locks");
    assert_eq!(locks["ok"], true);
    assert!(
        locks["detail"]
            .as_str()
            .expect("detail")
            .contains(&format!("{}: held", world.key("repo-"))),
        "{locks}"
    );
    drop(guard);
}

#[test]
fn a_gone_tree_and_a_missing_lock_file_are_reported_without_a_write() {
    let world = World::new();
    fs::remove_dir_all(world.dir.path().join("graph")).expect("remove tree");
    let lock = world.database("repo-").with_file_name("graph.db.lock");
    fs::remove_file(&lock).expect("remove lock");
    let data = doctor(world.data.path(), world.dir.path());
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(
        git.contains(&format!("{}: its tree is gone", world.key("graph-"))),
        "{git}"
    );
    assert!(!lock.exists(), "doctor made a lock file");
}
