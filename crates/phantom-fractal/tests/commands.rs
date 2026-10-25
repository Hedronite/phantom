//! `index` and `find` against a temp copy of `fixtures/index/repo`, with
//! the data directory in a second temp directory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use phantom_fractal_store::{SCHEMA_VERSION, Store};
use serde_json::Value;

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/index/repo");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-cli-{label}-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path.canonicalize().expect("canonical temp dir"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&dest).expect("create dir");
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("copy file");
        }
    }
}

/// Every path under `root` with its bytes; directories map to nothing.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            let rel = path.strip_prefix(root).expect("under root").to_path_buf();
            if path.is_dir() {
                out.insert(rel, None);
                stack.push(path);
            } else {
                out.insert(rel, Some(fs::read(&path).expect("read file")));
            }
        }
    }
    out
}

struct Fixture {
    repo: TempDir,
    data: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let repo = TempDir::new("repo");
        copy_tree(Path::new(FIXTURE), repo.path());
        Self {
            repo,
            data: TempDir::new("data"),
        }
    }

    fn run(&self, bin: &str, args: &[&str]) -> Output {
        Command::new(bin)
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .current_dir(self.repo.path())
            .output()
            .unwrap_or_else(|err| panic!("cannot run {bin}: {err}"))
    }

    fn index(&self, bin: &str) -> (Option<i32>, Value) {
        let repo = self.repo.path().to_str().expect("utf-8 temp path");
        envelope(&self.run(bin, &["index", repo]))
    }
}

fn envelope(out: &Output) -> (Option<i32>, Value) {
    let text = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(text.trim_end())
        .unwrap_or_else(|err| panic!("stdout is not one JSON envelope ({err}): {text}"));
    (out.status.code(), value)
}

#[test]
fn index_writes_a_stamped_database_outside_the_tree() {
    for bin in BINS {
        let fixture = Fixture::new();
        let before = snapshot(fixture.repo.path());
        let (code, env) = fixture.index(bin);
        assert_eq!(code, Some(0), "{bin}: {env}");
        assert_eq!(env["ok"], true, "{bin}: {env}");
        assert_eq!(env["schema"], 1, "{bin}: {env}");
        assert_eq!(env["error"], Value::Null, "{bin}: {env}");
        assert!(env["data"]["written"].as_u64() >= Some(1), "{bin}: {env}");

        let database = PathBuf::from(env["data"]["database"].as_str().expect("database"));
        assert!(database.starts_with(fixture.data.path()), "{bin}: {env}");
        assert!(!database.starts_with(fixture.repo.path()), "{bin}: {env}");
        let store = Store::open(&database).expect("open the index");
        assert_eq!(
            store.producer().expect("producer").as_deref(),
            Some("phantom-fractal")
        );
        assert_eq!(
            store.schema_version().expect("schema_version"),
            SCHEMA_VERSION
        );
        assert_eq!(
            snapshot(fixture.repo.path()),
            before,
            "{bin} changed the tree"
        );
    }
}

#[test]
fn a_second_index_writes_nothing() {
    let fixture = Fixture::new();
    let (_, first) = fixture.index(BINS[1]);
    let (code, second) = fixture.index(BINS[0]);
    assert_eq!(code, Some(0), "{second}");
    assert_eq!(second["data"]["written"], 0, "{second}");
    assert_eq!(
        second["data"]["unchanged"], first["data"]["written"],
        "{second}"
    );
}

#[test]
fn index_of_a_missing_path_is_not_found() {
    let fixture = Fixture::new();
    let missing = fixture.repo.path().join("no-such-dir");
    let out = fixture.run(BINS[1], &["index", missing.to_str().expect("utf-8")]);
    let (code, env) = envelope(&out);
    assert_eq!(code, Some(3), "{env}");
    assert_eq!(env["ok"], false, "{env}");
    assert_eq!(env["error"]["kind"], "not_found", "{env}");
    assert_eq!(env["error"]["exit"], 3, "{env}");
}

#[test]
fn index_refuses_a_data_directory_inside_the_tree() {
    let fixture = Fixture::new();
    let before = snapshot(fixture.repo.path());
    let inside = fixture.repo.path().join("phantom-data");
    let repo = fixture.repo.path().to_str().expect("utf-8");
    for data in [inside.clone(), PathBuf::from("phantom-data")] {
        let out = Command::new(BINS[1])
            .args(["index", repo])
            .env("PHANTOM_FRACTAL_DATA", &data)
            .current_dir(fixture.repo.path())
            .output()
            .expect("run fract");
        let (code, env) = envelope(&out);
        assert_eq!(code, Some(1), "{data:?}: {env}");
        assert_eq!(env["ok"], false, "{data:?}: {env}");
        assert!(!inside.exists(), "{data:?}: data directory was created");
    }
    assert_eq!(snapshot(fixture.repo.path()), before);
}

impl Fixture {
    fn find(&self, bin: &str, query: &str) -> (Option<i32>, Value) {
        envelope(&self.run(bin, &["find", query]))
    }
}

#[test]
fn find_returns_an_indexed_name() {
    let fixture = Fixture::new();
    fixture.index(BINS[1]);
    let before = snapshot(fixture.repo.path());
    for bin in BINS {
        let (code, env) = fixture.find(bin, "open_ledger");
        assert_eq!(code, Some(0), "{bin}: {env}");
        assert_eq!(env["ok"], true, "{bin}: {env}");
        assert_eq!(env["schema"], 1, "{bin}: {env}");
        let hits = env["data"].as_array().expect("data is a list");
        assert_eq!(hits.len(), 1, "{bin}: {env}");
        assert_eq!(hits[0]["name"], "open_ledger", "{bin}: {env}");
        assert_eq!(hits[0]["kind"], "function", "{bin}: {env}");
        assert_eq!(hits[0]["visibility"], "pub", "{bin}: {env}");
        assert_eq!(hits[0]["path"], "src/ledger.rs", "{bin}: {env}");
        assert_eq!(hits[0]["line"], 7, "{bin}: {env}");
    }
    assert_eq!(
        snapshot(fixture.repo.path()),
        before,
        "find changed the tree"
    );
}

#[test]
fn find_with_repo_works_from_another_directory() {
    let fixture = Fixture::new();
    fixture.index(BINS[0]);
    let repo = fixture.repo.path().to_str().expect("utf-8");
    let out = Command::new(BINS[1])
        .args(["find", "open_ledger", "--repo", repo])
        .env("PHANTOM_FRACTAL_DATA", fixture.data.path())
        .current_dir(fixture.data.path())
        .output()
        .expect("run fract");
    let (code, env) = envelope(&out);
    assert_eq!(code, Some(0), "{env}");
    assert_eq!(env["data"][0]["name"], "open_ledger", "{env}");
}

#[test]
fn find_of_an_absent_name_is_an_empty_list() {
    let fixture = Fixture::new();
    fixture.index(BINS[1]);
    let absent = [
        "no_such_symbol",
        "open",
        "Open_Ledger",
        "open_ledger\" OR name:open_ledger",
        "\"",
        "::",
        "*",
        "",
    ];
    for bin in BINS {
        for query in absent {
            let (code, env) = fixture.find(bin, query);
            assert_eq!(code, Some(0), "{bin} {query:?}: {env}");
            assert_eq!(env["ok"], true, "{bin} {query:?}: {env}");
            assert_eq!(
                env["data"],
                Value::Array(Vec::new()),
                "{bin} {query:?}: {env}"
            );
        }
    }
}

#[test]
fn find_without_an_index_is_no_index_and_creates_nothing() {
    let fixture = Fixture::new();
    let (code, env) = fixture.find(BINS[1], "open_ledger");
    assert_eq!(code, Some(2), "{env}");
    assert_eq!(env["ok"], false, "{env}");
    assert_eq!(env["error"]["kind"], "no_index", "{env}");
    let created = fs::read_dir(fixture.data.path()).expect("data dir").count();
    assert_eq!(created, 0, "find created files in the data directory");
}
