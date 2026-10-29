//! `fract mcp --root` and the one failure for every repo a tool may not
//! use (SPEC-w8 G0b).
//!
//! The working directory holds two indexed roots `a` and `b`, a root `c`
//! with no index, an indexed tree `outside` that is not a root, a plain
//! directory and a file. Every way of naming something that is not an
//! indexed root must give the same reply, byte for byte, with no path.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// The text item every refused `repo` gets.
const REFUSED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"repo is not an indexed root of this server\",\"exit\":2},\"schema\":1}";

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-mcp-roots-{label}-{}-{}",
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
    fs::create_dir_all(to).expect("create dir");
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("copy file");
        }
    }
}

struct World {
    cwd: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            cwd: TempDir::new("cwd"),
            data: TempDir::new("data"),
        };
        let cwd = world.cwd.path();
        for (name, fixture, indexed) in [
            ("a", "impact", true),
            ("b", "detail", true),
            ("c", "graph", false),
            ("outside", "graph", true),
        ] {
            copy_tree(&Path::new(FIXTURES).join(fixture), &cwd.join(name));
            if indexed {
                let output = world.cli(&["index", name]);
                assert_eq!(output.status.code(), Some(0));
            }
        }
        fs::create_dir_all(cwd.join("plain")).expect("plain dir");
        fs::write(cwd.join("notes.txt"), "not a tree\n").expect("file");
        world
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(self.cwd.path())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli")
    }

    /// The server's output for `calls`, run from `dir` with `args`.
    fn serve(&self, args: &[&str], dir: &Path, calls: &[(&str, Value)]) -> Output {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"roots\",\"version\":\"0\"}}}\n",
        );
        for (id, (name, arguments)) in (1..).zip(calls) {
            let message = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            });
            input.push_str(&message.to_string());
            input.push('\n');
        }
        let mut child = Command::new(BIN)
            .arg("mcp")
            .args(args)
            .current_dir(dir)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn server");
        // A server that refuses to start may close stdin first.
        let _ = child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes());
        child.wait_with_output().expect("wait")
    }

    /// Each call's text item, from a server that must serve them all.
    fn texts(&self, args: &[&str], dir: &Path, calls: &[(&str, Value)]) -> Vec<String> {
        let output = self.serve(args, dir, calls);
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let root = self.cwd.path().to_str().expect("utf-8");
        assert!(!stdout.contains(root), "a path was echoed: {stdout}");
        let replies: Vec<Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(replies.len(), calls.len() + 1);
        replies[1..]
            .iter()
            .map(|reply| {
                let text = reply["result"]["content"][0]["text"]
                    .as_str()
                    .expect("text");
                let parsed: Value = serde_json::from_str(text).expect("envelope");
                assert_eq!(reply["result"]["structuredContent"], parsed);
                text.to_owned()
            })
            .collect()
    }
}

/// Each read tool with arguments that succeed on an indexed `a`.
fn reads() -> [(&'static str, Value); 7] {
    [
        ("find_symbols", json!({ "query": "root_fn" })),
        ("show_symbol", json!({ "symbol": "root_fn" })),
        ("callers", json!({ "symbol": "root_fn" })),
        ("callees", json!({ "symbol": "run" })),
        ("impact", json!({ "symbol": "root_fn" })),
        ("top_ranked", json!({})),
        ("repo_map", json!({ "budget": 500 })),
    ]
}

fn with_repo(arguments: &Value, repo: Option<&str>) -> Value {
    let mut arguments = arguments.clone();
    if let Some(repo) = repo {
        arguments["repo"] = json!(repo);
    }
    arguments
}

#[test]
fn every_repo_that_is_not_an_indexed_root_gets_the_same_reply() {
    let world = World::new();
    let cwd = world.cwd.path();
    // A directory inside a root is refused even when it has an index of
    // its own.
    let output = world.cli(&["index", "a/engine"]);
    assert_eq!(output.status.code(), Some(0));
    let outside = cwd.join("outside");
    let refused: Vec<Option<&str>> = vec![
        Some("missing"),
        Some("plain"),
        Some("c"),
        Some("outside"),
        Some(outside.to_str().expect("utf-8")),
        Some("a/engine"),
        Some("."),
        Some(""),
        Some("notes.txt"),
        Some("a/../missing"),
        None,
    ];
    let mut calls = Vec::new();
    for (tool, arguments) in reads() {
        for repo in &refused {
            calls.push((tool, with_repo(&arguments, *repo)));
        }
    }
    let roots = ["--root", "a", "--root", "b", "--root", "c"];
    let texts = world.texts(&roots, cwd, &calls);
    for (text, (tool, arguments)) in texts.iter().zip(&calls) {
        assert_eq!(text, REFUSED, "{tool} {arguments}");
    }
    // Every root answers, however it is named, and a second `--root` is
    // as good as the first.
    let a = cwd.join("a");
    let mut calls = Vec::new();
    for repo in ["a", "./a", "a/engine/..", a.to_str().expect("utf-8")] {
        for (tool, arguments) in reads() {
            calls.push((tool, with_repo(&arguments, Some(repo))));
        }
    }
    calls.push(("find_symbols", json!({ "query": "largest", "repo": "b" })));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&a, cwd.join("link")).expect("symlink");
        calls.push((
            "find_symbols",
            json!({ "query": "root_fn", "repo": "link" }),
        ));
    }
    for text in world.texts(&roots, cwd, &calls) {
        assert!(text.starts_with("{\"ok\":true"), "{text}");
    }
}

#[test]
fn argument_errors_come_before_the_repo() {
    let world = World::new();
    let calls = [
        ("find_symbols", json!({ "repo": "missing" })),
        ("top_ranked", json!({ "limit": 0, "repo": "missing" })),
        (
            "impact",
            json!({ "symbol": "a", "summary": 1, "repo": "c" }),
        ),
        ("callers", json!({ "symbol": "a", "max_tokens": 0 })),
        ("repo_map", json!({ "repo": "outside" })),
    ];
    let texts = world.texts(&["--root", "a", "--root", "c"], world.cwd.path(), &calls);
    let kinds: Vec<Value> = texts
        .iter()
        .map(|text| serde_json::from_str::<Value>(text).expect("envelope")["error"]["kind"].clone())
        .collect();
    assert_eq!(kinds, vec![json!("usage"); calls.len()]);
}

#[test]
fn the_default_repo_is_the_only_root_or_the_working_directory() {
    let world = World::new();
    let cwd = world.cwd.path();
    let find = [("find_symbols", json!({ "query": "root_fn" }))];
    // One root that is not the working directory: it is the default.
    let only = world.texts(&["--root", "a"], cwd, &find);
    assert!(only[0].starts_with("{\"ok\":true"), "{}", only[0]);
    // Two roots, run from one of them: that one.
    let a = cwd.join("a");
    let b = cwd.join("b");
    let both = [
        "--root",
        a.to_str().expect("utf-8"),
        "--root",
        b.to_str().expect("utf-8"),
    ];
    let from_a = world.texts(&both, &a, &find);
    assert!(from_a[0].starts_with("{\"ok\":true"), "{}", from_a[0]);
    // Two roots, run from neither: no default.
    assert_eq!(world.texts(&both, cwd, &find), [REFUSED]);
    // No `--root`: the working directory, which here has no index.
    assert_eq!(world.texts(&[], cwd, &find), [REFUSED]);
    let from_root = world.texts(&[], &a, &find);
    assert!(from_root[0].starts_with("{\"ok\":true"), "{}", from_root[0]);
}

#[test]
fn a_root_that_is_not_a_directory_stops_the_server() {
    let world = World::new();
    let cwd = world.cwd.path();
    let missing = cwd.join("missing");
    for (args, message) in [
        (
            vec!["--root", "missing"],
            "--root 1 of 1 is not a directory",
        ),
        (
            vec!["--root", "a", "--root", missing.to_str().expect("utf-8")],
            "--root 2 of 2 is not a directory",
        ),
        (
            vec!["--allow-index", "--root", "notes.txt", "--root", "a"],
            "--root 1 of 2 is not a directory",
        ),
    ] {
        let output = world.serve(&args, cwd, &reads()[..1]);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).expect("utf-8");
        assert_eq!(stderr, format!("phantom-fractal mcp: usage: {message}\n"));
    }
}

#[test]
fn index_repo_indexes_only_a_root() {
    let world = World::new();
    let cwd = world.cwd.path();
    let databases = |world: &World| {
        let mut count = 0;
        let mut stack = vec![world.data.path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read data dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.ends_with("graph.db") {
                    count += 1;
                }
            }
        }
        count
    };
    let before = databases(&world);
    let calls = [
        ("index_repo", json!({ "path": "missing" })),
        ("index_repo", json!({ "path": "plain" })),
        ("index_repo", json!({ "path": "outside" })),
        ("index_repo", json!({ "path": "c/src" })),
        ("index_repo", json!({ "path": "notes.txt" })),
        ("find_symbols", json!({ "query": "main", "repo": "c" })),
    ];
    let texts = world.texts(&["--allow-index", "--root", "c"], cwd, &calls);
    assert_eq!(texts, vec![REFUSED; calls.len()]);
    assert_eq!(databases(&world), before, "a refusal writes nothing");
    let calls = [
        ("index_repo", json!({ "path": "c" })),
        ("top_ranked", json!({ "repo": "c", "limit": 1 })),
    ];
    let texts = world.texts(&["--allow-index", "--root", "c"], cwd, &calls);
    assert!(texts[0].starts_with("{\"ok\":true"), "{}", texts[0]);
    assert!(texts[1].starts_with("{\"ok\":true"), "{}", texts[1]);
    assert_eq!(databases(&world), before + 1);
}

#[test]
fn the_command_line_keeps_its_own_errors() {
    let world = World::new();
    for (args, line, code) in [
        (
            &["find", "root_fn", "--repo", "missing"][..],
            "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"not_found\",\"message\":\"repo path not found\",\"exit\":3},\"schema\":1}\n",
            3,
        ),
        (
            &["find", "root_fn", "--repo", "plain"][..],
            "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"no index for this path; run index first\",\"exit\":2},\"schema\":1}\n",
            2,
        ),
        (
            &["find", "root_fn", "--repo", "a/engine"][..],
            "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"no index for this path; run index first\",\"exit\":2},\"schema\":1}\n",
            2,
        ),
    ] {
        let output = world.cli(args);
        assert_eq!(output.status.code(), Some(code), "{args:?}");
        assert_eq!(String::from_utf8(output.stdout).expect("utf-8"), line);
    }
    let output = world.cli(&["find", "root_fn", "--repo", "outside"]);
    assert_eq!(output.status.code(), Some(0), "the CLI has no roots");
}
