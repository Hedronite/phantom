//! `fract watch` (SPEC-w9 G1). Each sequence starts a watch on a temp
//! tree with a fresh data directory, makes one change at a time, and
//! after each batch line checks that the watched database's
//! `canonical_dump` equals a fresh `fract index` of the same tree into a
//! separate data directory (D6, canon §18.2).
//!
//! The watch runs with short intervals, so the sequences stay quick; the
//! defaults are checked by the usage tests. Git histories are built by
//! the `git` CLI in `support`, in temp dirs, with fixed times and
//! identity, no system or global config, a temp `HOME` and no hooks.

mod support;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use phantom_fractal_store::Store;
use serde_json::Value;
use support::{HEAD, Repo, Script, TempDir, copy_fixture, database_path, snapshot};

const FRACT: &str = env!("CARGO_BIN_EXE_fract");
/// Longest a test waits for one batch line.
const BATCH_WAIT: Duration = Duration::from_secs(20);
/// How long a test listens for a line that must not come.
const QUIET: Duration = Duration::from_millis(1_200);
const INTERVAL_MS: &str = "100";
const DEBOUNCE_MS: &str = "150";

/// A tree, its watch, and the watch's data directory.
struct Watched {
    root: PathBuf,
    data: TempDir,
    child: Child,
    lines: Receiver<String>,
    batches: u64,
    /// What the fresh index each batch is compared with is given.
    index_args: Vec<&'static str>,
}

impl Watched {
    /// Starts `fract watch` on `root` with `extra` arguments, and waits for
    /// the batch of its first pass.
    fn start(root: &Path, extra: &[&str]) -> Self {
        let data = TempDir::new("watch-data");
        let mut watched = Self::spawn(root, data, DEBOUNCE_MS, extra);
        watched.batch();
        watched
    }

    fn spawn(root: &Path, data: TempDir, debounce_ms: &str, extra: &[&str]) -> Self {
        let mut child = Command::new(FRACT)
            .args(["watch", "--repo"])
            .arg(root)
            .args(["--interval-ms", INTERVAL_MS, "--debounce-ms", debounce_ms])
            .args(extra)
            .env("PHANTOM_FRACTAL_DATA", data.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn watch");
        let stdout = child.stdout.take().expect("stdout");
        let (send, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            root: root.to_path_buf(),
            data,
            child,
            lines,
            batches: 0,
            index_args: Vec::new(),
        }
    }

    /// The next batch line: numbered after the last, path-free, with the
    /// index names and `meta`, and a dump equal to a fresh index's.
    fn batch(&mut self) -> Value {
        let line = match self.lines.recv_timeout(BATCH_WAIT) {
            Ok(line) => line,
            Err(err) => panic!("no batch line: {err:?}"),
        };
        let value = check_line(&line, &self.root, self.data.path());
        self.batches += 1;
        assert_eq!(value["data"]["batch"], self.batches, "{line}");
        let fresh = fresh_dump_with(&self.root, &self.index_args);
        assert_same(&self.dump(), &fresh, self.batches);
        value
    }

    /// No line arrives for [`QUIET`].
    fn quiet(&self) {
        match self.lines.recv_timeout(QUIET) {
            Err(RecvTimeoutError::Timeout) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    fn dump(&self) -> String {
        dump(self.data.path())
    }

    /// The exit code once the watch exits, or none if it is still running
    /// after `wait`.
    fn exit_within(&mut self, wait: Duration) -> Option<i32> {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("try wait") {
                return status.code();
            }
            thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// Kills the watch, as a user would. There is no signal handler.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `line` parsed and checked: one compact success envelope with `data`
/// keys `batch, written, removed, rebuilt, cochange` and `meta` after
/// `data`, a fresh `meta`, and no path of the tree or the data dir.
fn check_line(line: &str, root: &Path, data: &Path) -> Value {
    assert!(!line.contains(&*root.to_string_lossy()), "{line}");
    assert!(!line.contains(&*data.to_string_lossy()), "{line}");
    assert!(
        !line.contains("/Users/") && !line.contains("/tmp/") && !line.contains("/home/"),
        "{line}"
    );
    assert!(
        !line.contains(": ") && !line.contains(", "),
        "not compact: {line}"
    );
    let value: Value = serde_json::from_str(line).expect("json line");
    assert_eq!(value["ok"], true, "{line}");
    assert_eq!(
        support::raw_keys(line, 0),
        ["ok", "data", "meta", "error", "schema"]
    );
    let at = support::value_at(line, "data", 0);
    assert_eq!(
        support::raw_keys(line, at),
        ["batch", "written", "removed", "rebuilt", "cochange"]
    );
    let meta = &value["meta"];
    assert_eq!(meta["stale"], false, "{line}");
    assert_eq!(meta["head"], meta["index_head"], "{line}");
    value
}

/// `got` equals `want`, or a panic that names the first line that
/// differs and its table.
fn assert_same(got: &str, want: &str, batch: u64) {
    if got == want {
        return;
    }
    let mut table = "";
    for (got_line, want_line) in got.lines().zip(want.lines()) {
        if let Some(name) = want_line.strip_prefix("# table ") {
            table = name;
        }
        assert_eq!(got_line, want_line, "after batch {batch}, table {table}");
    }
    assert_eq!(
        got.lines().count(),
        want.lines().count(),
        "after batch {batch}"
    );
}

/// The bytes and mtime of the one `graph.db` under `data`, and the bytes
/// of its write-ahead log, a missing log read as empty: any open, a read
/// included, makes an empty log where the last close removed it.
fn db_state(data: &Path) -> (Vec<u8>, std::time::SystemTime, Vec<u8>) {
    let db = database_path(data);
    let meta = fs::metadata(&db).expect("db meta");
    let wal = fs::read(db.with_extension("db-wal")).unwrap_or_default();
    (
        fs::read(&db).expect("db"),
        meta.modified().expect("mtime"),
        wal,
    )
}

/// The canonical dump of the one database under `data`.
fn dump(data: &Path) -> String {
    Store::open(&database_path(data))
        .expect("open store")
        .canonical_dump()
        .expect("dump")
}

/// The dump of a fresh `fract index` of `root` into a new data dir.
fn fresh_dump(root: &Path) -> String {
    fresh_dump_with(root, &[])
}

/// [`fresh_dump`], with `args` given to `index`.
fn fresh_dump_with(root: &Path, args: &[&str]) -> String {
    let data = TempDir::new("watch-fresh");
    let output = Command::new(FRACT)
        .arg("index")
        .args(args)
        .arg(root)
        .env("PHANTOM_FRACTAL_DATA", data.path())
        .output()
        .expect("run index");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    dump(data.path())
}

fn counts(batch: &Value) -> (u64, u64, bool) {
    let data = &batch["data"];
    (
        data["written"].as_u64().expect("written"),
        data["removed"].as_u64().expect("removed"),
        data["rebuilt"].as_bool().expect("rebuilt"),
    )
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(path, text).expect("write");
}

/// A one-package crate: `lib.rs` declares `a` and `b`, imports
/// `a::helper`, and calls into both.
fn package() -> TempDir {
    let dir = TempDir::new("watch-tree");
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub mod a;\npub mod b;\n\nuse crate::a::helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    write(
        root,
        "src/a.rs",
        "pub fn helper() {}\n\npub fn other() {}\n",
    );
    write(
        root,
        "src/b.rs",
        "pub fn run() {\n    crate::a::other();\n}\n",
    );
    dir
}

#[test]
fn add_a_fn() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    write(
        tree.path(),
        "src/b.rs",
        "pub fn run() {\n    crate::a::other();\n}\n\npub fn added() {\n    run();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (1, 0, true));
    w.quiet();
}

#[test]
fn rename_a_fn_used_elsewhere() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    write(
        tree.path(),
        "src/a.rs",
        "pub fn assist() {}\n\npub fn other() {}\n",
    );
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\n\nuse crate::a::assist;\n\npub fn top() {\n    assist();\n    b::run();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (2, 0, true));
}

#[test]
fn rename_a_fn_across_modules() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    write(tree.path(), "src/a.rs", "pub fn other() {}\n");
    write(
        tree.path(),
        "src/b.rs",
        "pub fn helper() {}\n\npub fn run() {\n    crate::a::other();\n}\n",
    );
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\n\nuse crate::b::helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (3, 0, true));
}

#[test]
fn delete_a_file() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    fs::remove_file(tree.path().join("src/b.rs")).expect("delete");
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\n\nuse crate::a::helper;\n\npub fn top() {\n    helper();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (1, 1, true));
}

#[test]
fn delete_a_file_then_restore_it() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    let text = fs::read_to_string(tree.path().join("src/b.rs")).expect("read");
    fs::remove_file(tree.path().join("src/b.rs")).expect("delete");
    assert_eq!(counts(&w.batch()), (0, 1, true));
    write(tree.path(), "src/b.rs", &text);
    assert_eq!(counts(&w.batch()), (1, 0, true));
}

#[test]
fn move_a_module() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    fs::rename(
        tree.path().join("src/a.rs"),
        tree.path().join("src/util.rs"),
    )
    .expect("move");
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod util;\npub mod b;\n\nuse crate::util::helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    write(
        tree.path(),
        "src/b.rs",
        "pub fn run() {\n    crate::util::other();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (3, 1, true));
}

#[test]
fn change_a_use() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\n\nuse crate::a::other as helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (1, 0, true));
}

#[test]
fn edit_cargo_toml() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    write(
        tree.path(),
        "Cargo.toml",
        "[package]\nname = \"renamed\"\nversion = \"0.2.0\"\nedition = \"2024\"\n",
    );
    assert_eq!(counts(&w.batch()), (0, 0, true));
}

#[test]
fn a_test_module_that_moves_out_of_cfg_test() {
    let tree = package();
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\n#[cfg(test)]\nmod checks;\n\nuse crate::a::helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    write(
        tree.path(),
        "src/checks.rs",
        "fn probe() {\n    crate::top();\n}\n",
    );
    let mut w = Watched::start(tree.path(), &[]);
    write(
        tree.path(),
        "src/lib.rs",
        "pub mod a;\npub mod b;\npub mod checks;\n\nuse crate::a::helper;\n\npub fn top() {\n    helper();\n    b::run();\n}\n",
    );
    assert_eq!(counts(&w.batch()), (1, 0, true));
}

#[test]
fn a_burst_of_twenty_saves_inside_the_debounce_is_one_batch() {
    let tree = package();
    let data = TempDir::new("watch-data");
    // A long debounce, so the whole burst falls inside it.
    let mut w = Watched::spawn(tree.path(), data, "1500", &[]);
    w.batch();
    let path = tree.path().join("src/a.rs");
    let started = Instant::now();
    for save in 0..20 {
        let text =
            format!("pub fn helper() {{}}\n\npub fn other() {{}}\n\npub fn save_{save}() {{}}\n");
        fs::write(&path, text).expect("save");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "the burst was slow"
    );
    assert_eq!(counts(&w.batch()), (1, 0, true));
    w.quiet();
    assert!(w.dump().contains("save_19"));
    assert!(!w.dump().contains("save_18"));
}

#[test]
fn saves_closer_than_the_debounce_settle_only_when_they_stop() {
    // The burst lasts longer than the debounce, but no gap between saves
    // reaches it: the same file stays modified throughout, and its size
    // and mtime keep the tree from settling until the last save.
    let tree = package();
    let data = TempDir::new("watch-data");
    let mut w = Watched::spawn(tree.path(), data, "400", &[]);
    w.batch();
    let path = tree.path().join("src/a.rs");
    let started = Instant::now();
    for save in 0..12 {
        let text =
            format!("pub fn helper() {{}}\n\npub fn other() {{}}\n\npub fn slow_{save}() {{}}\n");
        fs::write(&path, text).expect("save");
        thread::sleep(Duration::from_millis(120));
    }
    assert!(started.elapsed() > Duration::from_millis(1_200));
    assert_eq!(counts(&w.batch()), (1, 0, true));
    w.quiet();
    assert!(w.dump().contains("slow_11"));
}

#[test]
fn a_tree_that_vanishes_ends_the_watch_with_a_failure_and_no_path() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    fs::remove_dir_all(tree.path()).expect("remove tree");
    let line = w.lines.recv_timeout(BATCH_WAIT).expect("a failure line");
    assert_eq!(w.exit_within(BATCH_WAIT), Some(1), "{line}");
    let value: Value = serde_json::from_str(&line).expect("json");
    assert_eq!(value["ok"], false, "{line}");
    assert!(value.get("meta").is_none(), "{line}");
    assert!(!line.contains(&*tree.path().to_string_lossy()), "{line}");
}

#[test]
fn a_touch_alone_runs_no_pass() {
    let tree = package();
    let w = Watched::start(tree.path(), &[]);
    let before = w.dump();
    let databases = db_state(w.data.path());
    for rel in ["src/a.rs", "src/lib.rs", "Cargo.toml"] {
        let file = fs::File::options()
            .write(true)
            .open(tree.path().join(rel))
            .expect("open");
        let mtime = file.metadata().expect("meta").modified().expect("mtime");
        file.set_modified(mtime + Duration::from_secs(5))
            .expect("touch");
    }
    w.quiet();
    assert_eq!(w.dump(), before);
    assert!(
        db_state(w.data.path()) == databases,
        "a touch wrote the database"
    );
}

#[test]
fn a_fresh_tree_writes_nothing() {
    let tree = package();
    let data = TempDir::new("watch-data");
    let index = Command::new(FRACT)
        .arg("index")
        .arg(tree.path())
        .env("PHANTOM_FRACTAL_DATA", data.path())
        .output()
        .expect("index");
    assert!(index.status.success());
    let databases = db_state(data.path());
    let tree_before = snapshot(tree.path());
    let mut w = Watched::spawn(tree.path(), data, DEBOUNCE_MS, &["--max-batches", "1"]);
    // The test's own timeout: the watch has nothing to do and runs on.
    w.quiet();
    w.kill();
    assert!(
        db_state(w.data.path()) == databases,
        "a fresh watch wrote the database"
    );
    assert_eq!(snapshot(tree.path()), tree_before);
}

#[test]
fn max_batches_exits_zero_after_that_many() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &["--max-batches", "2"]);
    write(tree.path(), "src/a.rs", "pub fn helper() {}\n");
    w.batch();
    assert_eq!(w.exit_within(BATCH_WAIT), Some(0));
}

#[test]
fn a_held_lock_refuses_a_second_watch_and_index_with_exit_4() {
    let tree = package();
    let w = Watched::start(tree.path(), &[]);
    for args in [
        vec![
            "watch".into(),
            "--repo".into(),
            tree.path().as_os_str().to_owned(),
        ],
        vec!["index".into(), tree.path().as_os_str().to_owned()],
    ] {
        let started = Instant::now();
        let output = Command::new(FRACT)
            .args(&args)
            .env("PHANTOM_FRACTAL_DATA", w.data.path())
            .output()
            .expect("second writer");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(output.status.code(), Some(4), "{args:?}");
        let line = String::from_utf8(output.stdout).expect("utf-8");
        let value: Value = serde_json::from_str(line.trim()).expect("json");
        assert_eq!(value["error"]["kind"], "lock_held");
        assert!(value.get("meta").is_none());
        assert!(!line.contains(&*tree.path().to_string_lossy()));
    }
}

#[test]
fn a_kill_mid_watch_leaves_a_database_status_reads_and_index_completes() {
    let tree = package();
    let mut w = Watched::start(tree.path(), &[]);
    // Changes the watch may be in the middle of when it is killed.
    for n in 0..30 {
        write(
            tree.path(),
            &format!("src/gen_{n}.rs"),
            &format!("pub fn gen_{n}() {{}}\n"),
        );
    }
    thread::sleep(Duration::from_millis(300));
    w.kill();
    let data = &w.data;
    let status = Command::new(FRACT)
        .args(["status", "--repo"])
        .arg(tree.path())
        .env("PHANTOM_FRACTAL_DATA", data.path())
        .output()
        .expect("status");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let index = Command::new(FRACT)
        .arg("index")
        .arg(tree.path())
        .env("PHANTOM_FRACTAL_DATA", data.path())
        .output()
        .expect("index");
    assert!(
        index.status.success(),
        "{}",
        String::from_utf8_lossy(&index.stdout)
    );
    assert_eq!(dump(data.path()), fresh_dump(tree.path()));
}

#[test]
fn out_of_range_values_are_usage() {
    let tree = package();
    for args in [
        ["--interval-ms", "99"],
        ["--interval-ms", "10001"],
        ["--debounce-ms", "49"],
        ["--debounce-ms", "5001"],
        ["--max-batches", "0"],
        ["--max-batches", "1000001"],
    ] {
        let output = Command::new(FRACT)
            .args(["watch", "--repo"])
            .arg(tree.path())
            .args(args)
            .output()
            .expect("watch");
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
}

#[test]
fn a_fixture_copy_stays_equal_to_a_fresh_index_through_edits() {
    let dir = TempDir::new("watch-fixture");
    let root = dir.path().join("tree");
    copy_fixture("resolve-self", &root);
    let mut w = Watched::start(&root, &[]);
    let lib = root.join("src/lib.rs");
    let text = fs::read_to_string(&lib).expect("read");
    fs::write(&lib, format!("{text}// again\n")).expect("append");
    assert_eq!(counts(&w.batch()), (1, 0, true));
    fs::remove_file(root.join("src/shapes.rs")).expect("delete");
    assert_eq!(counts(&w.batch()), (0, 1, true));
    fs::write(&lib, text.replace("mod shapes;\n", "")).expect("drop mod");
    assert_eq!(counts(&w.batch()), (1, 0, true));
}

/// A git package with one commit on `main`.
fn git_package() -> Repo {
    let mut script = Script::default();
    script.commit(
        HEAD,
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n".to_owned(),
            ),
            (
                "src/lib.rs",
                "pub mod a;\n\npub fn top() {\n    a::helper();\n}\n".to_owned(),
            ),
            ("src/a.rs", "pub fn helper() {}\n".to_owned()),
        ],
    );
    Repo::new("watch-git", &script)
}

#[test]
fn a_git_commit_moves_head_and_rescans() {
    let repo = git_package();
    let mut w = Watched::start(&repo.root, &[]);
    let first = repo.rev("HEAD");
    assert_eq!(w.lines.try_recv().ok(), None);
    // A commit of a file the index keeps, then a commit that only moves
    // HEAD: the tree stays as checked out.
    let mut script = Script::after_main();
    script.commit(
        HEAD + 60,
        &[(
            "src/a.rs",
            "pub fn helper() {}\n\npub fn more() {}\n".to_owned(),
        )],
    );
    repo.import(&script);
    let batch = w.batch();
    assert_eq!(counts(&batch), (1, 0, true));
    let second = repo.rev("HEAD");
    assert_ne!(first, second);
    assert_eq!(batch["meta"]["head"], second);
    assert_eq!(batch["data"]["cochange"]["head"], second);
    let mut script = Script::after_main();
    script.commit(HEAD + 120, &[("README.md", "notes\n".to_owned())]);
    repo.import(&script);
    let batch = w.batch();
    assert_eq!(counts(&batch), (0, 0, false));
    assert_eq!(batch["meta"]["head"], repo.rev("HEAD"));
    assert_eq!(batch["data"]["cochange"]["commits_seen"], 3);
}

#[test]
fn the_window_index_was_given_is_kept() {
    let repo = git_package();
    let data = TempDir::new("watch-data");
    let index = Command::new(FRACT)
        .args(["index", "--cochange-days", "30", "--cochange-commits", "7"])
        .arg(&repo.root)
        .env("PHANTOM_FRACTAL_DATA", data.path())
        .output()
        .expect("index");
    assert!(index.status.success());
    let mut w = Watched::spawn(&repo.root, data, DEBOUNCE_MS, &[]);
    w.index_args = vec!["--cochange-days", "30", "--cochange-commits", "7"];
    let mut script = Script::after_main();
    script.commit(HEAD + 60, &[("README.md", "notes\n".to_owned())]);
    repo.import(&script);
    let batch = w.batch();
    assert_eq!(batch["data"]["batch"], 1);
    assert_eq!(batch["data"]["cochange"]["window"]["days"], 30);
    assert_eq!(batch["data"]["cochange"]["window"]["max_commits"], 7);
}

#[test]
fn a_git_branch_switch() {
    let repo = git_package();
    // A second branch with its own edit, imported before the watch.
    let mut stream = format!(
        "commit refs/heads/other\ncommitter Test <test@example.invalid> {} +0000\ndata 7\nmessage\nfrom refs/heads/main^0\n",
        HEAD + 30
    );
    for (path, body) in [
        ("src/a.rs", "pub fn helper() {}\n\npub fn branch() {}\n"),
        ("src/b.rs", "pub fn bee() {}\n"),
    ] {
        stream.push_str(&format!(
            "M 100644 inline {path}\ndata {}\n{body}\n",
            body.len()
        ));
    }
    repo.git(&["fast-import", "--quiet"], Some(stream.as_bytes()));
    let mut w = Watched::start(&repo.root, &[]);
    repo.git(&["checkout", "-q", "other"], None);
    let batch = w.batch();
    assert_eq!(counts(&batch), (2, 0, true));
    assert_eq!(batch["meta"]["head"], repo.rev("other"));
    repo.git(&["checkout", "-q", "main"], None);
    let batch = w.batch();
    assert_eq!(counts(&batch), (1, 1, true));
    assert_eq!(batch["meta"]["head"], repo.rev("main"));
}
