//! The registry (SPEC-w9 G3b, D10): `fract repos list`, `fract repos
//! remove KEY`, `--repo KEY`, and the MCP `repos` tool.
//!
//! Every tree is a fixture copy indexed into one shared data directory.
//! `remove` must delete exactly `<data>/repos/KEY/` and nothing else, and
//! no reply may hold a path.

mod support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use phantom_fractal_store::{Store, WriterLock};
use serde_json::{Value, json};

use support::{TempDir, copy_fixture, snapshot};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const NOT_A_KEY: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"a key is a lowercase name, a dash and 8 hex digits, as repos list prints it\",\"exit\":1},\"schema\":1}";

const UNKNOWN: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"not_found\",\"message\":\"no index has that key\",\"exit\":3},\"schema\":1}";

const LOCK_HELD: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"lock_held\",\"message\":\"store: another writer holds the database lock\",\"exit\":4},\"schema\":1}";

/// Fixture copies under one temp dir, and one data directory.
struct World {
    dir: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            dir: TempDir::new("registry-trees"),
            data: TempDir::new("registry-data"),
        }
    }

    /// A copy of fixture `fixture` named `name`, indexed. Its key.
    fn tree(&self, fixture: &str, name: &str) -> String {
        let root = self.dir.path().join(name);
        copy_fixture(fixture, &root);
        let (code, line) = self.run(&root, &["index", "."]);
        assert_eq!(code, Some(0), "{line}");
        self.keys()
            .into_iter()
            .find(|key| key.starts_with(&format!("{}-", name.to_lowercase().replace('_', "-"))))
            .expect("its key")
    }

    fn root(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn repos(&self) -> PathBuf {
        self.data.path().join("repos")
    }

    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = fs::read_dir(self.repos())
            .expect("repos")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8")
            })
            .collect();
        keys.sort();
        keys
    }

    fn run(&self, dir: &Path, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(BIN)
            .args(args)
            .current_dir(dir)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        (
            output.status.code(),
            stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned(),
        )
    }

    /// Run from the temp dir that holds the trees, which is no tree.
    fn line(&self, args: &[&str]) -> (Option<i32>, String) {
        self.run(self.dir.path(), args)
    }

    fn list(&self) -> Value {
        let (code, line) = self.line(&["repos", "list"]);
        assert_eq!(code, Some(0), "{line}");
        assert_eq!(
            self.line(&["repos"]),
            (code, line.clone()),
            "list is the default"
        );
        assert!(!line.contains("\"meta\""), "{line}");
        for path in [self.dir.path(), self.data.path()] {
            assert!(!line.contains(path.to_str().expect("utf-8")), "{line}");
        }
        assert!(!line.contains("\"/"), "{line}");
        serde_json::from_str::<Value>(&line).expect("envelope")["data"]["repos"].clone()
    }

    fn store(&self, key: &str) -> Store {
        // Read-only, so the expected values change nothing either.
        Store::open_read_only(&self.repos().join(key).join("graph.db")).expect("store")
    }

    fn entry(&self, key: &str, name: &str, stale: Value) -> Value {
        let store = self.store(key);
        json!({
            "key": key,
            "name": name,
            "stale": stale,
            "files": store.row_count("files").expect("files"),
            "symbols": store.row_count("symbols").expect("symbols"),
        })
    }
}

#[test]
fn list_is_sorted_by_key_and_says_what_is_stale_or_gone() {
    let world = World::new();
    let graph = world.tree("graph", "graph-copy");
    let impact = world.tree("impact", "impact-copy");
    // A name that is not its own slug: `name` is the tree's, not the key's.
    let detail = world.tree("detail", "Detail_Copy");
    // Directories that are not indexes are not listed.
    fs::create_dir_all(world.repos().join("not-a-key")).expect("dir");
    fs::write(world.repos().join("not-a-key/graph.db"), "x").expect("file");
    fs::create_dir_all(world.repos().join("empty-0123abcd")).expect("dir");
    fs::write(world.repos().join("stray-01234567"), "x").expect("file");
    let mut want = vec![
        world.entry(&detail, "Detail_Copy", json!(false)),
        world.entry(&graph, "graph-copy", json!(false)),
        world.entry(&impact, "impact-copy", json!(false)),
    ];
    want.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    assert_eq!(world.list(), json!(want));

    // `list` writes nothing at all: not a byte, a mode, or an mtime of a
    // file or a directory.
    let before = snapshot(world.data.path());
    fs::write(
        world.root("graph-copy").join("app/src/extra.rs"),
        "pub fn e() {}\n",
    )
    .expect("edit");
    fs::remove_dir_all(world.root("impact-copy")).expect("remove a tree");
    let listed = world.list();
    let by_key = |key: &str| {
        listed
            .as_array()
            .expect("repos")
            .iter()
            .find(|entry| entry["key"] == key)
            .cloned()
            .expect("listed")
    };
    assert_eq!(by_key(&graph)["stale"], true);
    assert_eq!(
        by_key(&impact),
        world.entry(&impact, "impact-copy", Value::Null)
    );
    assert_eq!(by_key(&detail)["stale"], false);
    assert_eq!(snapshot(world.data.path()), before, "list wrote");

    // An index in use, with a WAL beside it, is not opened: what it holds
    // is null, and it is still listed by key.
    let wal = world.repos().join(&detail).join("graph.db-wal");
    fs::write(&wal, b"").expect("a WAL");
    let in_use = snapshot(world.data.path());
    let listed = world.list();
    let entry = listed
        .as_array()
        .expect("repos")
        .iter()
        .find(|entry| entry["key"] == detail.as_str())
        .cloned()
        .expect("listed");
    assert_eq!(
        entry,
        json!({ "key": detail, "name": "detail-copy", "stale": null, "files": null, "symbols": null })
    );
    assert_eq!(snapshot(world.data.path()), in_use, "list wrote");
    fs::remove_file(&wal).expect("remove");
}

#[test]
fn remove_deletes_exactly_one_index_directory() {
    let world = World::new();
    let graph = world.tree("graph", "graph-copy");
    let impact = world.tree("impact", "impact-copy");
    // A link inside the index directory, to a directory outside it.
    let outside = TempDir::new("registry-outside");
    fs::write(outside.path().join("keep.txt"), "keep").expect("keep");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), world.repos().join(&graph).join("link"))
        .expect("link");
    let other = snapshot(&world.repos().join(&impact));
    let trees = snapshot(world.dir.path());
    let kept = snapshot(outside.path());
    assert_eq!(
        world.line(&["repos", "remove", &graph]),
        (
            Some(0),
            format!(
                "{{\"ok\":true,\"data\":{{\"key\":\"{graph}\",\"removed\":true}},\"error\":null,\"schema\":1}}"
            )
        )
    );
    assert_eq!(world.keys(), std::slice::from_ref(&impact));
    assert_eq!(snapshot(&world.repos().join(&impact)), other);
    assert_eq!(snapshot(world.dir.path()), trees, "the tree is untouched");
    assert_eq!(
        snapshot(outside.path()),
        kept,
        "the link's target is untouched"
    );
    // Removed once, it is unknown.
    assert_eq!(
        world.line(&["repos", "remove", &graph]),
        (Some(3), UNKNOWN.to_owned())
    );
}

#[test]
fn remove_refuses_bad_keys_unknown_keys_links_and_a_held_lock() {
    let world = World::new();
    let graph = world.tree("graph", "graph-copy");
    for key in [
        "Graph-12345678",
        "graph-1234567",
        "graph-123456789",
        "graph-1234567g",
        "-12345678",
        "graph12345678",
        "../graph-12345678",
        "graph/../x-12345678",
        ".",
        "..",
        "",
        "/tmp/x-12345678",
    ] {
        assert_eq!(
            world.line(&["repos", "remove", "--", key]),
            (Some(1), NOT_A_KEY.to_owned()),
            "{key}"
        );
    }
    assert_eq!(
        world.line(&["repos", "remove", "nope-00000000"]),
        (Some(3), UNKNOWN.to_owned())
    );
    // A key that is a link out of `repos/`, or a file, is no index.
    let outside = TempDir::new("registry-outside");
    fs::write(outside.path().join("graph.db"), "x").expect("bait");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), world.repos().join("link-0123abcd")).expect("link");
    fs::write(world.repos().join("file-0123abcd"), "x").expect("file");
    let with_links = snapshot(world.data.path());
    let bait = snapshot(outside.path());
    for key in ["link-0123abcd", "file-0123abcd"] {
        assert_eq!(
            world.line(&["repos", "remove", "--", key]),
            (Some(3), UNKNOWN.to_owned()),
            "{key}"
        );
    }
    assert_eq!(snapshot(outside.path()), bait);
    assert_eq!(snapshot(world.data.path()), with_links);
    fs::remove_file(world.repos().join("link-0123abcd")).expect("unlink");
    fs::remove_file(world.repos().join("file-0123abcd")).expect("rm");

    // A held lock refuses, and leaves everything in place.
    let guard = WriterLock::acquire(&world.repos().join(&graph).join("graph.db")).expect("lock");
    let held = snapshot(world.data.path());
    assert_eq!(
        world.line(&["repos", "remove", &graph]),
        (Some(4), LOCK_HELD.to_owned())
    );
    assert_eq!(snapshot(world.data.path()), held);
    drop(guard);
    assert_eq!(world.line(&["repos", "remove", &graph]).0, Some(0));
}

#[test]
fn repo_takes_a_key_as_well_as_a_path() {
    let world = World::new();
    let graph = world.tree("graph", "graph-copy");
    let root = world.root("graph-copy");
    let by_path = |args: &[&str]| {
        let mut args = args.to_vec();
        let path = root.to_str().expect("utf-8");
        args.extend(["--repo", path]);
        world.line(&args)
    };
    let by_key = |args: &[&str]| {
        let mut args = args.to_vec();
        args.extend(["--repo", graph.as_str()]);
        world.line(&args)
    };
    for args in [
        &["find", "deep"][..],
        &["callers", "deep"],
        &["impact", "deep"],
        &["show", "all"],
        &["rank"],
        &["map", "--budget", "200"],
        &["status"],
        &["path", "all", "deep"],
    ] {
        let want = by_path(args);
        assert_eq!(want.0, Some(0), "{args:?}: {}", want.1);
        assert_eq!(by_key(args), want, "{args:?}");
    }
    // An index in use, with a WAL beside it, still resolves.
    let wal = world.repos().join(&graph).join("graph.db-wal");
    fs::write(&wal, b"").expect("a WAL");
    assert_eq!(by_key(&["find", "deep"]), by_path(&["find", "deep"]));
    fs::remove_file(&wal).ok();
    // An unknown key is a missing path, as before.
    let (code, line) = world.line(&["find", "deep", "--repo", "nope-00000000"]);
    assert_eq!(code, Some(3), "{line}");
    // A path that exists wins over a key of the same name.
    fs::create_dir_all(world.dir.path().join(&graph)).expect("same-named dir");
    let (code, line) = world.line(&["find", "deep", "--repo", &graph]);
    assert_eq!(code, Some(2), "{line}");
}

#[test]
fn the_mcp_repos_tool_lists_only_indexed_roots() {
    let world = World::new();
    let graph = world.tree("graph", "graph-copy");
    let _impact = world.tree("impact", "impact-copy");
    let bare = world.root("bare");
    fs::create_dir_all(&bare).expect("bare");
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"repos\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
    );
    for (id, arguments) in [(2, json!({})), (3, json!({ "repo": "." }))] {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "repos", "arguments": arguments },
        });
        input.push_str(&format!("{message}\n"));
    }
    let mut child = Command::new(BIN)
        .args(["mcp", "--root"])
        .arg(world.root("graph-copy"))
        .arg("--root")
        .arg(&bare)
        .current_dir(world.dir.path())
        .env("PHANTOM_FRACTAL_DATA", world.data.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("line"))
        .collect();
    let tools = replies[1]["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    let at = names
        .iter()
        .position(|name| *name == "repos")
        .expect("listed");
    assert_eq!(names[at - 1], "repo_status");
    assert_eq!(
        tools[at]["inputSchema"],
        json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false })
    );
    let text = replies[2]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert_eq!(
        replies[2]["result"]["structuredContent"],
        serde_json::from_str::<Value>(text).expect("json")
    );
    // Only the indexed root: not the bare root, and not the index that is
    // no root of this server.
    let want = json!({
        "ok": true,
        "data": { "repos": [world.entry(&graph, "graph-copy", json!(false))] },
        "error": null,
        "schema": 1,
    });
    assert_eq!(serde_json::from_str::<Value>(text).expect("json"), want);
    assert!(
        !text.contains(world.dir.path().to_str().expect("utf-8")),
        "{text}"
    );
    let refused = replies[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert_eq!(
        refused,
        "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"this tool takes no arguments\",\"exit\":1},\"schema\":1}"
    );
    // `repos remove` is not a tool.
    assert!(!names.iter().any(|name| name.contains("remove")));
}
