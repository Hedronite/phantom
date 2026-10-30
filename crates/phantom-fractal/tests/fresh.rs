//! Freshness at the command line (SPEC-w9 G0c): `meta` on every success
//! envelope and on no failure, `fract status`, the writer lock `index`
//! holds, and the `root` and `index_head` keys it stores.
//!
//! A read must answer from a stale index without re-indexing it, so every
//! read here leaves the database and its WAL as they were.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use phantom_fractal_store::{Store, WriterLock};
use serde_json::{Value, json};

use support::{
    Repo, Script, TempDir, copy_fixture, database, database_path, history, raw_keys, snapshot,
    strip_meta, value_at,
};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const LOCK_HELD: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"lock_held\",\"message\":\"store: another writer holds the database lock\",\"exit\":4},\"schema\":1}";

/// A tree to index, and a data directory outside it.
struct World {
    /// Keeps the fixture copy alive; a git world keeps its [`Repo`].
    _dir: Option<TempDir>,
    repo: Option<Repo>,
    root: PathBuf,
    data: TempDir,
}

impl World {
    /// A copy of the `graph` fixture, which has no git, in a directory
    /// named `graph-tree`.
    fn fixture() -> Self {
        let dir = TempDir::new("fresh-tree");
        let root = dir.path().join("graph-tree");
        copy_fixture("graph", &root);
        Self {
            _dir: Some(dir),
            repo: None,
            root,
            data: TempDir::new("fresh-data"),
        }
    }

    /// The co-change `history` as a git root.
    fn git() -> Self {
        let repo = Repo::new("fresh", &history());
        Self {
            _dir: None,
            root: repo.root.clone(),
            repo: Some(repo),
            data: TempDir::new("fresh-data"),
        }
    }

    fn repo(&self) -> &Repo {
        self.repo.as_ref().expect("a git world")
    }

    /// The exit code and the one stdout line, newline removed, run from
    /// the root.
    fn line_with(&self, bin: &str, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(bin)
            .args(args)
            .current_dir(&self.root)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let line = stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned();
        assert!(!line.contains('\n'), "{line}");
        (output.status.code(), line)
    }

    fn line(&self, args: &[&str]) -> (Option<i32>, String) {
        self.line_with(BINS[0], args)
    }

    /// The envelope of a command that must succeed.
    fn ok(&self, args: &[&str]) -> Value {
        self.ok_with(BINS[0], args)
    }

    fn ok_with(&self, bin: &str, args: &[&str]) -> Value {
        let (code, line) = self.line_with(bin, args);
        assert_eq!(code, Some(0), "{args:?}: {line}");
        serde_json::from_str(&line).expect("envelope")
    }

    fn index(&self) -> Value {
        self.ok(&["index", "."])
    }

    fn store(&self) -> Store {
        Store::open(&database_path(self.data.path())).expect("open store")
    }

    /// The bytes and mtimes of the database and its WAL.
    fn db(&self) -> Vec<(PathBuf, Vec<u8>, Option<std::time::SystemTime>)> {
        database(self.data.path())
    }
}

/// The success commands of a world whose tree has `fa`, `top` and
/// `src/a.rs`, or, on the fixture, `deep`, `all` and `app/src/uses.rs`.
fn reads(git: bool) -> Vec<Vec<&'static str>> {
    let (symbol, caller, file) = if git {
        ("fa", "top", "src/a.rs")
    } else {
        ("deep", "all", "app/src/uses.rs")
    };
    vec![
        vec!["find", symbol],
        vec!["find", symbol, "--max-tokens", "2000"],
        vec!["callers", symbol],
        vec!["callees", caller],
        vec!["impact", symbol],
        vec!["impact", symbol, "--summary", "--max-tokens", "2000"],
        vec!["show", symbol],
        vec!["show", symbol, "--source", "--max-tokens", "2000"],
        vec!["rank"],
        vec!["rank", "--files", "--max-tokens", "2000"],
        vec!["map", "--budget", "300"],
        vec!["cochange", file],
        vec!["status"],
    ]
}

/// `meta` is the third key, after `ok` and `data`, with exactly its four
/// keys in order; `want` is its value.
fn assert_meta(line: &str, want: &Value) {
    assert_eq!(
        raw_keys(line, 0),
        ["ok", "data", "meta", "error", "schema"],
        "{line}"
    );
    let meta_at = value_at(line, "meta", value_at(line, "data", 0));
    let meta_at = line[..meta_at].rfind("\"meta\":").expect("meta") + "\"meta\":".len();
    assert_eq!(
        raw_keys(line, meta_at),
        ["stale", "head", "index_head", "rank_epoch"],
        "{line}"
    );
    let parsed: Value = serde_json::from_str(line).expect("envelope");
    assert_eq!(&parsed["meta"], want, "{line}");
}

fn meta(stale: bool, head: Option<&str>, index_head: Option<&str>, rank_epoch: u64) -> Value {
    json!({ "stale": stale, "head": head, "index_head": index_head, "rank_epoch": rank_epoch })
}

#[test]
fn every_success_carries_meta_after_data_and_no_failure_does() {
    for git in [false, true] {
        let world = if git { World::git() } else { World::fixture() };
        let head = git.then(|| world.repo().rev("HEAD"));
        let (code, line) = world.line(&["index", "."]);
        assert_eq!(code, Some(0), "{line}");
        let rank_epoch = world.store().rank_epoch().expect("rank epoch");
        let fresh = meta(false, head.as_deref(), head.as_deref(), rank_epoch);
        assert_meta(&line, &fresh);
        for bin in BINS {
            for args in reads(git) {
                let (code, line) = world.line_with(bin, &args);
                assert_eq!(code, Some(0), "{args:?}: {line}");
                assert_meta(&line, &fresh);
            }
        }

        // Failures and `version` carry no `meta`.
        let elsewhere = TempDir::new("fresh-elsewhere");
        let unindexed = World {
            _dir: None,
            repo: None,
            root: world.root.clone(),
            data: elsewhere,
        };
        let failures: [(&World, &[&str], i32); 7] = [
            (&world, &["show", "no_such_symbol"], 3),
            (&world, &["cochange", "no/such.rs"], 3),
            (&world, &["find", "x", "--repo", "no/such/dir"], 3),
            (&world, &["index", "no/such/dir"], 3),
            (&world, &["find", "x", "--max-tokens", "1"], 1),
            (&unindexed, &["find", "x"], 2),
            (&unindexed, &["status"], 2),
        ];
        for (world, args, exit) in failures {
            let (code, line) = world.line(args);
            assert_eq!(code, Some(exit), "{args:?}: {line}");
            assert_eq!(
                raw_keys(&line, 0),
                ["ok", "data", "error", "schema"],
                "{args:?}: {line}"
            );
        }
        let (code, line) = world.line(&["version"]);
        assert_eq!(code, Some(0));
        assert_eq!(raw_keys(&line, 0), ["ok", "data", "error", "schema"]);
    }
}

#[test]
fn stale_follows_the_tree_and_head_and_reads_never_reindex() {
    let world = World::git();
    let repo = world.repo();
    world.index();
    let first = repo.rev("HEAD");
    let epoch = world.store().rank_epoch().expect("rank epoch");
    let probe = |want: Value, changes: [u64; 3], fingerprint: bool| {
        let db = world.db();
        let tree = snapshot(&world.root);
        for args in [&["find", "fa"][..], &["rank"], &["status"]] {
            let line = world.line(args).1;
            assert_meta(&line, &want);
        }
        let status = world.ok(&["status"])["data"].clone();
        assert_eq!(
            status["changes"],
            json!({ "added": changes[0], "modified": changes[1], "removed": changes[2] }),
            "{status}"
        );
        assert_eq!(status["fingerprint_changed"], fingerprint, "{status}");
        assert_eq!(status["stale"], want["stale"], "{status}");
        assert_eq!(world.db(), db, "a read wrote the database");
        assert_eq!(snapshot(&world.root), tree, "a read wrote the tree");
    };
    let fresh = meta(false, Some(&first), Some(&first), epoch);
    let stale = meta(true, Some(&first), Some(&first), epoch);
    probe(fresh.clone(), [0, 0, 0], false);

    // An edit, then the same bytes again with a new mtime.
    let path = world.root.join("src/b.rs");
    let original = fs::read(&path).expect("read b");
    fs::write(&path, b"pub fn fb() { let _ = 1; }\n").expect("edit b");
    probe(stale.clone(), [0, 1, 0], false);
    fs::write(&path, &original).expect("restore b");
    probe(fresh.clone(), [0, 0, 0], false);

    // An added file, then a removed one.
    fs::write(world.root.join("src/new.rs"), "pub fn fresh() {}\n").expect("add");
    probe(stale.clone(), [1, 0, 0], false);
    fs::remove_file(world.root.join("src/new.rs")).expect("remove new");
    fs::remove_file(world.root.join("src/g.rs")).expect("remove g");
    probe(stale.clone(), [0, 0, 1], false);
    repo.git(&["checkout", "--", "src/g.rs"], None);
    probe(fresh.clone(), [0, 0, 0], false);

    // A manifest edit changes only the fingerprint.
    let manifest = world.root.join("Cargo.toml");
    let cargo = fs::read_to_string(&manifest).expect("manifest");
    fs::write(&manifest, format!("{cargo}\n[features]\nextra = []\n")).expect("edit manifest");
    probe(stale.clone(), [0, 0, 0], true);
    fs::write(&manifest, &cargo).expect("restore manifest");
    probe(fresh, [0, 0, 0], false);

    // A commit that changes no indexed file moves HEAD alone.
    let mut script = Script::after_main();
    script.commit(
        support::HEAD + support::DAY,
        &[("README.md", "notes\n".to_owned())],
    );
    repo.import(&script);
    let second = repo.rev("HEAD");
    assert_ne!(first, second);
    probe(
        meta(true, Some(&second), Some(&first), epoch),
        [0, 0, 0],
        false,
    );

    // An index pass brings it back, and the pass after that writes nothing.
    let line = world.line(&["index", "."]).1;
    let epoch = world.store().rank_epoch().expect("rank epoch");
    assert_meta(&line, &meta(false, Some(&second), Some(&second), epoch));
    let db = world.db();
    world.index();
    assert_eq!(world.db(), db, "a second pass wrote");
}

#[test]
fn index_stores_the_root_and_head_once() {
    for git in [false, true] {
        let world = if git { World::git() } else { World::fixture() };
        world.index();
        let store = world.store();
        let root = world.root.canonicalize().expect("canonical root");
        assert_eq!(
            store.root().expect("root"),
            Some(root.to_string_lossy().into_owned())
        );
        let head = git.then(|| world.repo().rev("HEAD"));
        assert_eq!(store.index_head().expect("index head"), head);
        drop(store);
        let db = world.db();
        world.index();
        assert_eq!(world.db(), db, "git {git}: a second pass wrote");
    }
}

#[test]
fn status_has_the_spec_keys_and_no_path() {
    for git in [false, true] {
        let world = if git { World::git() } else { World::fixture() };
        let indexed = world.index();
        let (code, line) = world.line(&["status"]);
        assert_eq!(code, Some(0), "{line}");
        let data_at = value_at(&line, "data", 0);
        assert_eq!(
            raw_keys(&line, data_at),
            [
                "key",
                "name",
                "head",
                "index_head",
                "stale",
                "changes",
                "fingerprint_changed",
                "counts",
                "versions",
                "epochs",
                "cochange",
                "lock"
            ],
            "{line}"
        );
        for (key, want) in [
            ("changes", &["added", "modified", "removed"][..]),
            ("counts", &["files", "symbols", "edges"]),
            ("versions", &["schema", "extractor", "envelope"]),
            ("epochs", &["graph", "rank"]),
        ] {
            assert_eq!(
                raw_keys(&line, value_at(&line, key, data_at)),
                want,
                "{line}"
            );
        }
        let root = world.root.canonicalize().expect("canonical root");
        for absolute in [&root, &world.root, &world.data.path().to_path_buf()] {
            assert!(!line.contains(absolute.to_str().expect("utf-8")), "{line}");
        }
        assert!(!line.contains("\"/"), "{line}");

        let data = &serde_json::from_str::<Value>(&line).expect("envelope")["data"];
        let database = database_path(world.data.path());
        let key = database
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .expect("key");
        assert_eq!(data["key"], key);
        assert!(
            key.starts_with(if git { "repo-" } else { "graph-tree-" }),
            "{key}"
        );
        assert_eq!(data["name"], if git { "repo" } else { "graph-tree" });
        let store = world.store();
        let head = git.then(|| world.repo().rev("HEAD"));
        assert_eq!(data["head"], json!(head));
        assert_eq!(data["index_head"], json!(head));
        assert_eq!(data["stale"], false);
        assert_eq!(data["fingerprint_changed"], false);
        assert_eq!(
            data["counts"],
            json!({
                "files": store.row_count("files").expect("files"),
                "symbols": store.row_count("symbols").expect("symbols"),
                "edges": store.row_count("edges").expect("edges"),
            })
        );
        assert_eq!(
            data["versions"],
            json!({ "schema": 3, "extractor": 7, "envelope": 1 })
        );
        assert_eq!(
            data["epochs"],
            json!({
                "graph": store.graph_epoch().expect("graph"),
                "rank": store.rank_epoch().expect("rank"),
            })
        );
        // The scan as `index` printed it, or null without git.
        assert_eq!(data["cochange"], indexed["data"]["cochange"]);
        assert_eq!(data["cochange"].is_null(), !git);
        assert_eq!(data["lock"], "free");
        // Both binary names print it alike.
        assert_eq!(world.line_with(BINS[1], &["status"]), (Some(0), line));
    }
}

#[test]
fn a_held_lock_refuses_index_with_exit_4_and_reads_still_answer() {
    let world = World::git();
    world.index();
    let database = database_path(world.data.path());
    let lock_file = database.with_file_name("graph.db.lock");
    assert!(lock_file.exists(), "index made the lock file");
    assert_eq!(fs::metadata(&lock_file).expect("lock").len(), 0);
    let guard = WriterLock::acquire(&database).expect("take the lock");
    let db = world.db();
    for bin in BINS {
        let started = Instant::now();
        assert_eq!(
            world.line_with(bin, &["index", "."]),
            (Some(4), LOCK_HELD.to_owned())
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    let status = world.ok(&["status"]);
    assert_eq!(status["data"]["lock"], "held");
    assert_eq!(status["meta"]["stale"], false);
    assert_eq!(world.line(&["find", "fa"]).0, Some(0));
    assert_eq!(world.db(), db, "a refused index wrote");
    drop(guard);
    assert_eq!(world.ok(&["status"])["data"]["lock"], "free");
    world.index();
    assert_eq!(fs::metadata(&lock_file).expect("lock").len(), 0);
}

/// Every file under `data` but SQLite's shared-memory file, which readers
/// may touch, with its bytes and mtime. Directory mtimes are left out: an
/// open checks the owner with a file it makes and removes.
fn files(data: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime, Vec<u8>)> {
    snapshot(data)
        .into_iter()
        .filter(|(path, dir, ..)| !dir && !path.to_string_lossy().ends_with("-shm"))
        .map(|(path, _, len, mtime, bytes)| (path, len, mtime, bytes))
        .collect()
}

#[test]
fn status_writes_nothing_and_makes_no_lock_file() {
    let world = World::fixture();
    world.index();
    let lock_file = database_path(world.data.path()).with_file_name("graph.db.lock");
    fs::remove_file(&lock_file).expect("remove the lock file");
    let before = files(world.data.path());
    for bin in BINS {
        assert_eq!(world.ok_with(bin, &["status"])["data"]["lock"], "free");
    }
    assert_eq!(files(world.data.path()), before);
    assert!(!lock_file.exists(), "status made no lock file");
}

#[test]
fn bytes_without_meta_are_the_same_for_a_fresh_and_a_stale_index() {
    let world = World::fixture();
    world.index();
    let fresh: Vec<String> = reads(false)
        .iter()
        .filter(|args| args[0] != "status")
        .map(|args| world.line(args).1)
        .collect();
    fs::write(world.root.join("app/src/extra.rs"), "pub fn extra() {}\n").expect("add");
    let stale: Vec<String> = reads(false)
        .iter()
        .filter(|args| args[0] != "status")
        .map(|args| world.line(args).1)
        .collect();
    for (fresh, stale) in fresh.iter().zip(&stale) {
        assert!(stale.contains("\"meta\":{\"stale\":true"), "{stale}");
        if !fresh.contains("\"tokens\":") {
            assert_eq!(strip_meta(fresh), strip_meta(stale));
        }
    }
}

#[test]
fn two_writers_at_once_never_both_write_and_the_second_waits_or_gets_4() {
    // Each round starts with no database, so both writers race to create
    // it, then the same race on the index the round made.
    for round in 0..4 {
        let world = World::fixture();
        for existing in [false, true] {
            if existing {
                fs::write(
                    world.root.join(format!("app/src/round{round}.rs")),
                    "pub fn r() {}\n",
                )
                .expect("edit");
            }
            let spawn = || {
                Command::new(BINS[0])
                    .args(["index", "."])
                    .current_dir(&world.root)
                    .env("PHANTOM_FRACTAL_DATA", world.data.path())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .expect("spawn index")
            };
            let children = [spawn(), spawn()];
            let mut outcomes: Vec<(Option<i32>, String)> = children
                .into_iter()
                .map(|child| {
                    let output = child.wait_with_output().expect("wait");
                    let line = String::from_utf8(output.stdout).expect("utf-8");
                    (output.status.code(), line.trim_end().to_owned())
                })
                .collect();
            outcomes.sort();
            assert_eq!(outcomes[0].0, Some(0), "{outcomes:?}");
            match outcomes[1].0 {
                Some(0) => {}
                Some(4) => assert_eq!(outcomes[1].1, LOCK_HELD),
                _ => panic!("round {round} existing {existing}: {outcomes:?}"),
            }
            assert_eq!(world.ok(&["status"])["data"]["stale"], false);
        }
    }
}

#[cfg(unix)]
#[test]
fn a_tree_the_check_cannot_read_is_stale_and_still_answers() {
    use std::os::unix::fs::PermissionsExt;
    let world = World::fixture();
    world.index();
    // A changed size makes the check read the file, and the read fails.
    let path = world.root.join("app/src/uses.rs");
    fs::write(&path, "pub fn all() -> usize { 0 }\n").expect("edit");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
    let db = world.db();
    let line = world.line(&["find", "deep"]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod back");
    assert_eq!(line.0, Some(0), "{}", line.1);
    let rank_epoch = world.store().rank_epoch().expect("rank epoch");
    // Run as root, the file still reads, and the edit alone is stale.
    assert_meta(&line.1, &meta(true, None, None, rank_epoch));
    assert_eq!(world.db(), db);
}

#[test]
fn status_gives_the_stored_versions_where_reads_refuse_the_index() {
    let world = World::fixture();
    world.index();
    world
        .store()
        .set_extractor_version(6)
        .expect("stamp an older extractor");
    let status = world.ok(&["status"]);
    assert_eq!(
        status["data"]["versions"],
        json!({ "schema": 3, "extractor": 6, "envelope": 1 })
    );
    let (code, line) = world.line(&["callers", "deep"]);
    assert_eq!(code, Some(2), "{line}");
}
