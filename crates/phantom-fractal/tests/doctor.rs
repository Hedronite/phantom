//! `fract doctor` (SPEC-w9 G3b, D11): seven checks in a fixed order,
//! always exit 0, no path, and no write: not a byte, a mode or an mtime
//! under the data directory changes, and nothing is repaired. After the
//! checks, `agents` says whether `nu` and the skill are found (SPEC-w14
//! G2c), and never changes `ok`.

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

/// An entry's mtime, and for a file its length and bytes.
type Content = (std::time::SystemTime, Option<(u64, Vec<u8>)>);

/// `data` and every entry under it: path, kind, mode, and its
/// [`Content`]. Directory mtimes count, so a file made and removed again
/// shows. SQLite's shared memory is not there to start with, and never
/// made by `doctor`.
fn state(data: &Path) -> Vec<(PathBuf, bool, u32, Option<Content>)> {
    let mut out = Vec::new();
    if let Ok(meta) = fs::symlink_metadata(data) {
        out.push((
            data.to_path_buf(),
            true,
            0,
            Some((meta.modified().expect("mtime"), None)),
        ));
    }
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
            let bytes = (!meta.is_dir()).then(|| (meta.len(), fs::read(&path).unwrap_or_default()));
            let content = Some((meta.modified().expect("mtime"), bytes));
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
    let mut keys: Vec<&str> = data
        .as_object()
        .expect("data")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["agents", "checks", "ok"]);
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
    assert_eq!(check(&data, "extractor")["detail"], "13");
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
        databases.contains(&format!("{repo}: schema 4")),
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
    // The read-only open narrows nothing, so the index is still checked.
    assert!(
        git.contains(&format!("{}: not a git root", world.key("graph-"))),
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
            .contains("schema 99, newer than this binary's 4"),
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
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(
        git.contains(&format!(
            "{}: not checked, the index is in use",
            world.key("repo-")
        )),
        "{git}"
    );
    drop(guard);
}

#[test]
fn an_index_with_a_wal_beside_it_is_in_use_and_never_opened() {
    let world = World::new();
    let wal = world.database("graph-").with_file_name("graph.db-wal");
    fs::write(&wal, b"").expect("a WAL beside the index");
    let data = doctor(world.data.path(), world.dir.path());
    let git = check(&data, "git")["detail"]
        .as_str()
        .expect("git")
        .to_owned();
    assert!(
        git.contains(&format!(
            "{}: not checked, the index is in use",
            world.key("graph-")
        )),
        "{git}"
    );
    assert!(
        git.contains(&format!("{}: HEAD reads", world.key("repo-"))),
        "{git}"
    );
    assert_eq!(fs::read(&wal).expect("wal"), b"");
    fs::remove_file(&wal).expect("remove");
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

#[cfg(unix)]
#[test]
fn a_data_directory_others_can_write_and_an_open_repos_directory_are_flagged() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::new();
    let data_dir = world.data.path();
    let mode = |path: &Path| fs::metadata(path).expect("meta").permissions().mode() & 0o777;
    let original = mode(data_dir);
    fs::set_permissions(data_dir, fs::Permissions::from_mode(0o777)).expect("chmod data");
    let data = doctor(data_dir, world.dir.path());
    assert_eq!(data["ok"], false);
    let check_data = check(&data, "data_dir");
    assert_eq!(check_data["ok"], false);
    assert_eq!(
        check_data["detail"],
        "the data directory is 0777, writable by others"
    );
    assert_eq!(mode(data_dir), 0o777, "doctor narrowed the data directory");
    fs::set_permissions(data_dir, fs::Permissions::from_mode(original)).expect("chmod back");

    let repos = data_dir.join("repos");
    fs::set_permissions(&repos, fs::Permissions::from_mode(0o755)).expect("chmod repos");
    let data = doctor(data_dir, world.dir.path());
    let check_data = check(&data, "data_dir");
    assert_eq!(check_data["ok"], false);
    assert_eq!(
        check_data["detail"],
        "repos/ is 0755, not a private directory"
    );
    assert_eq!(mode(&repos), 0o755, "doctor narrowed repos/");
}

#[test]
fn a_database_that_is_not_sqlite_or_has_an_older_schema_is_reported() {
    let world = World::new();
    // An older binary's stamp, in `user_version` at byte 60 of the header.
    let older = world.database("repo-");
    let mut bytes = fs::read(&older).expect("read");
    bytes[60..64].copy_from_slice(&2_u32.to_be_bytes());
    fs::write(&older, &bytes).expect("write");
    let data = doctor(world.data.path(), world.dir.path());
    let databases = check(&data, "databases");
    assert_eq!(databases["ok"], true, "{databases}");
    let detail = databases["detail"].as_str().expect("detail").to_owned();
    assert!(
        detail.contains(&format!(
            "{}: schema 2, which index migrates to 4",
            world.key("repo-")
        )),
        "{detail}"
    );
    // The version before this one migrates the same way.
    bytes[60..64].copy_from_slice(&3_u32.to_be_bytes());
    fs::write(&older, &bytes).expect("write");
    let data = doctor(world.data.path(), world.dir.path());
    let databases = check(&data, "databases");
    assert_eq!(databases["ok"], true, "{databases}");
    let detail = databases["detail"].as_str().expect("detail").to_owned();
    assert!(
        detail.contains(&format!(
            "{}: schema 3, which index migrates to 4",
            world.key("repo-")
        )),
        "{detail}"
    );

    fs::write(world.database("graph-"), b"not a database").expect("write");
    let data = doctor(world.data.path(), world.dir.path());
    assert_eq!(data["ok"], false);
    let databases = check(&data, "databases");
    assert_eq!(databases["ok"], false);
    let detail = databases["detail"].as_str().expect("detail").to_owned();
    assert!(
        detail.contains(&format!("{}: not an SQLite database", world.key("graph-"))),
        "{detail}"
    );
}

/// `doctor` run in `dir` with exactly `env` set, besides the data
/// directory: its exit code and `data`.
fn doctor_with(dir: &Path, env: &[(&str, &std::ffi::OsStr)]) -> (Option<i32>, String, Value) {
    let mut command = Command::new(BINS[0]);
    command
        .arg("doctor")
        .current_dir(dir)
        .env_clear()
        .env("PHANTOM_FRACTAL_DATA", dir.join("data"));
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run doctor");
    let line = String::from_utf8(output.stdout).expect("utf-8");
    let data = serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone();
    (output.status.code(), line, data)
}

fn place(path: &Path, mode: u32) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(path, "").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("mode");
    }
    #[cfg(not(unix))]
    let _ = mode;
}

#[test]
fn agents_reports_nu_and_the_skill_and_never_changes_ok() {
    let root = TempDir::new("doctor-agents");
    let (project, home, bin) = (
        root.path().join("project"),
        root.path().join("home"),
        root.path().join("bin"),
    );
    fs::create_dir_all(&project).expect("project");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&bin).expect("bin");

    // Nothing to find: both false, and `doctor` is still ok, exit 0.
    let (code, line, data) = doctor_with(
        &project,
        &[("PATH", bin.as_os_str()), ("HOME", home.as_os_str())],
    );
    assert_eq!(code, Some(0), "{line}");
    assert_eq!(data["ok"], true, "{data}");
    assert_eq!(
        data["agents"],
        serde_json::json!({
            "nu": { "found": false, "via": "PATH" },
            "skill": { "found": false, "places": [] },
        })
    );

    // A `nu` on PATH, the skill in the project and a copy at home.
    place(&bin.join("nu"), 0o755);
    for dir in [
        project.join(".agents/skills/phantom"),
        home.join(".claude/skills/phantom"),
    ] {
        place(&dir.join("SKILL.md"), 0o644);
        place(&dir.join("phantom.nu"), 0o644);
    }
    let (code, line, data) = doctor_with(
        &project,
        &[("PATH", bin.as_os_str()), ("HOME", home.as_os_str())],
    );
    assert_eq!(code, Some(0), "{line}");
    assert_eq!(data["ok"], true, "{data}");
    assert_eq!(
        data["agents"],
        serde_json::json!({
            "nu": { "found": true, "via": "PATH" },
            "skill": {
                "found": true,
                "places": ["./.agents/skills/phantom", "~/.claude/skills/phantom"],
            },
        })
    );
    for path in [&project, &home, &bin] {
        assert!(!line.contains(path.to_str().expect("utf-8")), "{line}");
    }

    // PHANTOM_NU naming nothing wins over PATH, and is still no fault.
    let absent = root.path().join("absent");
    let (code, _, data) = doctor_with(
        &project,
        &[
            ("PATH", bin.as_os_str()),
            ("HOME", home.as_os_str()),
            ("PHANTOM_NU", absent.as_os_str()),
        ],
    );
    assert_eq!(code, Some(0));
    assert_eq!(data["ok"], true, "{data}");
    assert_eq!(
        data["agents"]["nu"],
        serde_json::json!({ "found": false, "via": "PHANTOM_NU" })
    );
}
