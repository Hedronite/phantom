//! Co-change at `index`, `fract cochange` and `show` `cochange`
//! (SPEC-w8 G3a), on synthetic git roots built by `support` and on
//! fixtures, which have no git.
//!
//! The scan `index` stores is checked against `phantom_fractal_git::mine`
//! on the same root, and `cochange` against the store's own
//! `cochange_of`. `index` must run no `git`, write nothing in the tree or
//! `.git`, and mine only when HEAD or the window moved; every read must
//! leave the database and its WAL as they were.

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use phantom_fractal_git::{Window, mine};
use phantom_fractal_store::Store;
use serde_json::{Value, json};

use support::{DAY, HEAD, Repo, Script, TempDir, database, history, module, snapshot, star};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A git root with [`history`] and a data directory to index it into.
struct World {
    repo: Repo,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        Self {
            repo: Repo::new("history", &history()),
            data: TempDir::new("data"),
        }
    }

    fn run_in(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(dir)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli")
    }

    /// The exit code and the one stdout line, newline removed, from the
    /// root.
    fn line(&self, args: &[&str]) -> (Option<i32>, String) {
        let output = self.run_in(&self.repo.root, args);
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let line = stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned();
        assert!(!line.contains('\n'), "{line}");
        (output.status.code(), line)
    }

    /// `data` of a command that must succeed.
    fn data(&self, args: &[&str]) -> Value {
        let (code, line) = self.line(args);
        assert_eq!(code, Some(0), "{args:?}: {line}");
        serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone()
    }

    /// The one `graph.db` under the data directory.
    fn store(&self) -> Store {
        let (path, _, _) = database(self.data.path())
            .into_iter()
            .find(|(path, _, _)| path.ends_with("graph.db"))
            .expect("graph.db");
        Store::open(&self.data.path().join(path)).expect("open store")
    }
}

/// Every indexed path the store holds.
fn indexed(store: &Store) -> BTreeSet<String> {
    store
        .files()
        .expect("files")
        .into_iter()
        .map(|file| file.path)
        .collect()
}

/// The keys of the object that starts at `at` in `raw`, at its own depth,
/// in the order they were printed. A parsed `Value` sorts its keys.
fn raw_keys(raw: &str, at: usize) -> Vec<String> {
    let bytes = raw.as_bytes();
    assert_eq!(bytes[at], b'{', "no object at {at}: {raw}");
    let (mut depth, mut keys, mut i) = (0usize, Vec::new(), at);
    let mut expect_key = false;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => {
                depth += 1;
                expect_key = bytes[i] == b'{' && depth == 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b',' if depth == 1 => expect_key = true,
            b'"' => {
                let start = i + 1;
                i += 1;
                while bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if expect_key && depth == 1 {
                    keys.push(raw[start..i].to_owned());
                    expect_key = false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// Where the value of the first `"key":` at or after `from` starts.
fn value_at(raw: &str, key: &str, from: usize) -> usize {
    let needle = format!("\"{key}\":");
    from + raw[from..]
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key}"))
        + needle.len()
}

/// `data.cochange` of `index` for `scan`, as SPEC-w8 G3a spells it.
fn summary(scan: &phantom_fractal_git::Scan, window: Window) -> Value {
    json!({
        "head": scan.last_oid,
        "commits_seen": scan.commits_seen,
        "skipped_bulk": scan.skipped_bulk,
        "pairs": scan.pairs.len(),
        "window": { "days": window.days, "max_commits": window.max_commits },
    })
}

#[test]
fn index_mines_once_per_head_and_window() {
    let world = World::new();
    let first = world.data(&["index", "."]);
    let store = world.store();
    let window = Window::default();
    let mined = mine(&world.repo.root, &window, &indexed(&store))
        .expect("mine")
        .expect("a git root");
    assert_eq!(
        mined.last_oid.as_deref(),
        Some(world.repo.rev("HEAD").as_str())
    );
    assert!(mined.pairs.len() >= 4, "{:?}", mined.pairs);
    assert_eq!(first["cochange"], summary(&mined, window));
    let stored = store.cochange_scan().expect("scan").expect("stored");
    assert_eq!(stored.window_json, window.to_json());
    assert_eq!(stored.pairs.len(), mined.pairs.len());
    drop(store);

    // The same HEAD and window: nothing is written, and the summary is the
    // stored one.
    let before = database(world.data.path());
    let (code, again) = world.line(&["index", "."]);
    assert_eq!(code, Some(0));
    assert_eq!(
        serde_json::from_str::<Value>(&again).expect("index")["data"]["cochange"],
        first["cochange"]
    );
    assert_eq!(
        database(world.data.path()),
        before,
        "an unchanged pass writes"
    );

    // Another window rescans, and the next pass with it writes nothing.
    let narrow = Window {
        days: 10,
        max_commits: 5000,
    };
    let data = world.data(&["index", ".", "--cochange-days", "10"]);
    let store = world.store();
    let mined = mine(&world.repo.root, &narrow, &indexed(&store))
        .expect("mine")
        .expect("a git root");
    assert_eq!(data["cochange"], summary(&mined, narrow));
    assert_ne!(
        data["cochange"]["commits_seen"],
        first["cochange"]["commits_seen"]
    );
    drop(store);
    let before = database(world.data.path());
    world.data(&["index", ".", "--cochange-days", "10"]);
    assert_eq!(database(world.data.path()), before);
    let capped = world.data(&["index", ".", "--cochange-commits", "3"]);
    assert_eq!(capped["cochange"]["commits_seen"], 3);
    assert_eq!(
        capped["cochange"]["window"],
        json!({ "days": 365, "max_commits": 3 })
    );

    // A new commit moves HEAD, and that rescans, with the window as the
    // last pass left it.
    world.data(&["index", "."]);
    let mut next = Script::after_main();
    let (path, body) = module("f", 99);
    next.commit(HEAD + DAY, &[(path.as_str(), body)]);
    world.repo.import(&next);
    let moved = world.data(&["index", "."]);
    assert_eq!(moved["cochange"]["head"], world.repo.rev("HEAD"));
    assert_ne!(moved["cochange"]["head"], first["cochange"]["head"]);
    assert_eq!(
        world.store().cochange_head().expect("head"),
        Some(world.repo.rev("HEAD"))
    );
}

#[test]
fn an_unmoved_head_is_not_mined_again() {
    // Removing an indexed file without a commit changes what a new mine
    // would count, so a pass that mined again would drop its history. The
    // stored scan keeps it: HEAD and the window did not move.
    let world = World::new();
    world.data(&["index", "."]);
    let has_g = |world: &World| {
        world
            .store()
            .cochange_scan()
            .expect("scan")
            .expect("stored")
            .files
            .iter()
            .any(|file| file.path == "src/g.rs")
    };
    assert!(has_g(&world));
    fs::remove_file(world.repo.root.join("src/g.rs")).expect("remove g");
    let data = world.data(&["index", "."]);
    assert_eq!(data["written"], 0, "{data}");
    assert!(has_g(&world), "the scan was mined again");
    // Another window does mine again, without the file.
    world.data(&["index", ".", "--cochange-days", "300"]);
    assert!(!has_g(&world));
}

#[test]
fn a_root_without_git_has_no_cochange() {
    // A fixture has no git, and a directory inside a git root is not a
    // root: neither is mined, and `data` has no `cochange` key.
    let tree = TempDir::new("fixture");
    let data = TempDir::new("data");
    copy_tree(&Path::new(FIXTURES).join("graph"), tree.path());
    let world = World::new();
    for (dir, path) in [(tree.path(), "."), (world.repo.root.as_path(), "src")] {
        let output = Command::new(BIN)
            .args(["index", path])
            .current_dir(dir)
            .env("PHANTOM_FRACTAL_DATA", data.path())
            .output()
            .expect("index");
        let line = String::from_utf8(output.stdout).expect("utf-8");
        assert_eq!(output.status.code(), Some(0), "{line}");
        assert!(!line.contains("cochange"), "{line}");
        let keys = raw_keys(&line, value_at(&line, "data", 0));
        assert_eq!(keys.last().map(String::as_str), Some("edges_by_reason"));
    }
}

#[test]
fn a_root_that_loses_its_git_loses_its_scan() {
    let world = World::new();
    world.data(&["index", "."]);
    assert!(world.data(&["show", "fa"])["cochange"].is_array());
    let git = world.repo.root.join(".git");
    let moved = world.repo.root.with_file_name("git-moved");
    fs::rename(&git, &moved).expect("move .git");
    let data = world.data(&["index", "."]);
    assert!(data.get("cochange").is_none(), "{data}");
    assert!(world.data(&["show", "fa"]).get("cochange").is_none());
    assert_eq!(world.data(&["cochange", "src/a.rs"])["scan"], Value::Null);
    let (_, map) = world.line(&["map", "--budget", "400"]);
    assert!(
        !map.contains("also-changes") && !map.contains("| head "),
        "{map}"
    );
    // The scan is cleared once; the next pass writes nothing.
    let before = database(world.data.path());
    world.data(&["index", "."]);
    assert_eq!(database(world.data.path()), before);
    fs::rename(&moved, &git).expect("restore .git");
    assert!(world.data(&["index", "."])["cochange"].is_object());
}

#[test]
fn the_window_flags_take_their_ranges() {
    let world = World::new();
    for args in [
        ["--cochange-days", "0"],
        ["--cochange-days", "3651"],
        ["--cochange-commits", "0"],
        ["--cochange-commits", "50001"],
        ["--cochange-days", "x"],
    ] {
        let output = world.run_in(&world.repo.root, &[&["index", "."][..], &args[..]].concat());
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    for args in [
        ["--cochange-days", "1"],
        ["--cochange-days", "3650"],
        ["--cochange-commits", "1"],
        ["--cochange-commits", "50000"],
    ] {
        let data = world.data(&[&["index", "."][..], &args[..]].concat());
        assert!(data["cochange"].is_object(), "{args:?}");
    }
}

#[test]
fn cochange_lists_partners_as_the_store_orders_them() {
    let world = World::new();
    world.data(&["index", "."]);
    let store = world.store();
    let head = world.repo.rev("HEAD");
    let (code, line) = world.line(&["cochange", "./src/a.rs"]);
    assert_eq!(code, Some(0), "{line}");
    let data_at = value_at(&line, "data", 0);
    assert_eq!(
        raw_keys(&line, data_at),
        ["file", "scan", "partners", "truncated"]
    );
    let data = &serde_json::from_str::<Value>(&line).expect("cochange")["data"];
    assert_eq!(data["file"], "src/a.rs");
    assert_eq!(
        data["scan"],
        json!({ "head": head, "window": { "days": 365, "max_commits": 5000 } })
    );
    let want = |min_lift: f64, limit: u32| -> Vec<Value> {
        store
            .cochange_of("src/a.rs", min_lift, limit)
            .expect("cochange_of")
            .into_iter()
            .map(|partner| {
                json!({
                    "path": partner.path,
                    "together": partner.together,
                    "lift": partner.lift,
                    "conf": partner.conf_of,
                    "last_time": partner.last_time,
                })
            })
            .collect()
    };
    let partners = data["partners"].as_array().expect("partners");
    assert!(partners.len() >= 2, "{line}");
    assert_eq!(*partners, want(1.5, 20));
    assert_eq!(data["truncated"], false);
    let partner_at = value_at(&line, "partners", data_at) + 1;
    assert_eq!(
        raw_keys(&line, partner_at),
        ["path", "together", "lift", "conf", "last_time"]
    );
    // The limit cuts the list and says so; a lower floor lets more in.
    let count = partners.len().to_string();
    let exact = world.data(&["cochange", "src/a.rs", "--limit", &count]);
    assert_eq!(exact["truncated"], false, "a limit of exactly the count");
    let one = world.data(&["cochange", "src/a.rs", "--limit", "1"]);
    assert_eq!(one["partners"], json!(want(1.5, 1)));
    assert_eq!(one["truncated"], true);
    let all = world.data(&["cochange", "src/a.rs", "--min-lift", "0", "--limit", "100"]);
    assert_eq!(all["partners"], json!(want(0.0, 100)));
    let high = world.data(&["cochange", "src/a.rs", "--min-lift", "1000"]);
    assert_eq!(high["partners"], json!([]));
    // An indexed file with no partners, and the same run twice.
    assert_eq!(
        world.data(&["cochange", "src/lib.rs"])["partners"],
        json!([])
    );
    assert_eq!(
        world.line(&["cochange", "src/a.rs"]),
        world.line(&["cochange", "src/a.rs"])
    );
}

#[test]
fn cochange_exits_are_usage_no_index_and_not_found() {
    let world = World::new();
    let (code, line) = world.line(&["cochange", "src/a.rs"]);
    assert_eq!(code, Some(2), "no index: {line}");
    world.data(&["index", "."]);
    let root = world.repo.root.to_str().expect("utf-8");
    let absolute = format!("{root}/src/a.rs");
    for file in ["src/nope.rs", "src", "Cargo.toml.bak", absolute.as_str()] {
        let (code, line) = world.line(&["cochange", file]);
        assert_eq!(code, Some(3), "{file}: {line}");
        assert!(!line.contains(root), "{line}");
    }
    for args in [
        &["cochange", "src/a.rs", "--limit", "0"][..],
        &["cochange", "src/a.rs", "--limit", "101"][..],
        &["cochange", "src/a.rs", "--min-lift", "-1"][..],
        &["cochange", "src/a.rs", "--min-lift", "NaN"][..],
        &["cochange", "src/a.rs", "--min-lift", "inf"][..],
        &["cochange", "src/a.rs", "--max-tokens", "0"][..],
        &["cochange"][..],
    ] {
        let (code, line) = world.line(args);
        assert_eq!((code, line.as_str()), (Some(1), ""), "{args:?}");
    }
    // A tree without git has no scan: `scan` is null and nothing is
    // listed.
    let tree = TempDir::new("fixture");
    copy_tree(&Path::new(FIXTURES).join("graph"), tree.path());
    let output = world.run_in(tree.path(), &["index", "."]);
    assert_eq!(output.status.code(), Some(0));
    let output = world.run_in(tree.path(), &["cochange", "app/src/dup.rs"]);
    let line = String::from_utf8(output.stdout).expect("utf-8");
    assert_eq!(
        line,
        "{\"ok\":true,\"data\":{\"file\":\"app/src/dup.rs\",\"scan\":null,\"partners\":[],\"truncated\":false},\
         \"meta\":{\"stale\":false,\"head\":null,\"index_head\":null,\"rank_epoch\":1},\"error\":null,\"schema\":1}\n"
    );
}

#[test]
fn show_lists_cochange_only_with_a_scan() {
    let world = World::new();
    world.data(&["index", "."]);
    let (code, line) = world.line(&["show", "fa"]);
    assert_eq!(code, Some(0), "{line}");
    assert_eq!(
        raw_keys(&line, value_at(&line, "data", 0)),
        [
            "symbol",
            "members",
            "callers",
            "callees",
            "macro_args",
            "cochange"
        ]
    );
    let shown = &serde_json::from_str::<Value>(&line).expect("show")["data"]["cochange"];
    let listed = world.data(&["cochange", "src/a.rs", "--limit", "5"]);
    assert_eq!(*shown, listed["partners"]);
    assert!(!shown.as_array().expect("partners").is_empty());
    let source = world.data(&["show", "fa", "--source"]);
    assert_eq!(source["cochange"], *shown);
    assert_eq!(source["source"]["status"], "ok");
    // At most 5, with the default floor.
    let all = world.data(&["cochange", "src/a.rs", "--limit", "100"]);
    assert!(shown.as_array().expect("partners").len() <= 5);
    assert!(
        all["partners"].as_array().expect("partners").len()
            >= shown.as_array().expect("partners").len()
    );
    // A file with 8 partners shows 5, the first 5 `cochange` lists.
    let spokes = World {
        repo: Repo::new("star", &star()),
        data: TempDir::new("data"),
    };
    spokes.data(&["index", "."]);
    let listed = spokes.data(&["cochange", "src/hub.rs"]);
    assert_eq!(listed["partners"].as_array().expect("partners").len(), 8);
    let shown = spokes.data(&["show", "fhub"]);
    assert_eq!(
        shown["cochange"],
        spokes.data(&["cochange", "src/hub.rs", "--limit", "5"])["partners"]
    );
    // No scan, no key.
    let tree = TempDir::new("fixture");
    copy_tree(&Path::new(FIXTURES).join("impact"), tree.path());
    assert_eq!(
        world.run_in(tree.path(), &["index", "."]).status.code(),
        Some(0)
    );
    let output = world.run_in(tree.path(), &["show", "root_fn", "--source"]);
    let line = String::from_utf8(output.stdout).expect("utf-8");
    assert!(!line.contains("cochange"), "{line}");
}

#[cfg(unix)]
#[test]
fn index_runs_no_git_and_writes_nothing_in_the_repo() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    // A `git` first on PATH that leaves a marker if anything runs it.
    let bin = TempDir::new("bin");
    let marker = bin.path().join("ran");
    let fake = bin.path().join("git");
    fs::write(
        &fake,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .expect("fake git");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let tree = snapshot(&world.repo.root);
    for args in [
        &["index", "."][..],
        &["index", "."][..],
        &["index", ".", "--cochange-days", "5"][..],
        &["cochange", "src/a.rs"][..],
        &["show", "fa", "--source"][..],
        &["map", "--budget", "500"][..],
    ] {
        let output = Command::new(BIN)
            .args(args)
            .current_dir(&world.repo.root)
            .env("PHANTOM_FRACTAL_DATA", world.data.path())
            .env("PATH", bin.path())
            .output()
            .expect("run cli");
        assert_eq!(output.status.code(), Some(0), "{args:?}");
    }
    assert!(!marker.exists(), "Phantom ran git");
    assert_eq!(snapshot(&world.repo.root), tree, "the tree or .git changed");
    let data = world.data(&["index", "."]);
    assert!(data["cochange"]["pairs"].as_u64().expect("pairs") > 0);
}

#[test]
fn reads_leave_the_database_alone() {
    let world = World::new();
    world.data(&["index", "."]);
    let before = database(world.data.path());
    assert!(!before.is_empty());
    for args in [
        &["cochange", "src/a.rs"][..],
        &[
            "cochange",
            "src/a.rs",
            "--min-lift",
            "0",
            "--max-tokens",
            "200",
        ][..],
        &["show", "fa", "--source"][..],
        &["show", "fa", "--max-tokens", "150"][..],
        &["map", "--budget", "600"][..],
        &["map", "--budget", "600", "--detail", "full"][..],
        &["rank"][..],
        &["rank", "--files"][..],
        &["find", "fa"][..],
        &["callers", "fa"][..],
        &["callees", "top"][..],
        &["impact", "fa"][..],
    ] {
        world.line(args);
    }
    assert_eq!(database(world.data.path()), before, "a CLI read wrote");
    let calls = [
        ("cochange", json!({ "file": "src/a.rs" })),
        ("show_symbol", json!({ "symbol": "fa" })),
        ("repo_map", json!({ "budget": 600 })),
        ("top_ranked", json!({ "kind": "files" })),
        ("find_symbols", json!({ "query": "fa" })),
    ];
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"reads\",\"version\":\"0\"}}}\n",
    );
    for (id, (name, arguments)) in (1..).zip(&calls) {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        input.push_str(&format!("{message}\n"));
    }
    let output = serve(&world, &input);
    assert_eq!(output.lines().count(), calls.len() + 1);
    assert!(!output.contains("\"isError\":true"), "{output}");
    assert_eq!(database(world.data.path()), before, "an MCP read wrote");
}

/// The server's stdout for `input`, run from the root.
fn serve(world: &World, input: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(&world.repo.root)
        .env("PHANTOM_FRACTAL_DATA", world.data.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(0));
    String::from_utf8(output.stdout).expect("utf-8")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create dir");
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let dest: PathBuf = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("copy file");
        }
    }
}
