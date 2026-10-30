//! `fract path` and the MCP `symbol_path` tool (SPEC-w9 G3b, D9), on a
//! small generated package with calls, a type's method, and two symbols of
//! one name.
//!
//! Each reply is checked against `phantom_fractal_analyze::path` run on the
//! same store, and each tool result against the command line's line.

mod support;

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use phantom_fractal_analyze::path;
use phantom_fractal_store::Store;
use serde_json::{Value, json};

use support::{TempDir, database, database_path, raw_keys, value_at};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const REFUSED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"repo is not an indexed root of this server\",\"exit\":2},\"schema\":1}";

/// `start` calls `a::step`, which calls `b::finish`; `Hub::run` calls
/// `a::step`; `dup` is in `a` and in `b`.
struct World {
    tree: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            tree: TempDir::new("path-tree"),
            data: TempDir::new("path-data"),
        };
        let root = world.tree.path();
        fs::create_dir_all(root.join("src")).expect("src");
        for (path, body) in [
            (
                "Cargo.toml",
                "[package]\nname = \"paths\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
            ),
            (
                "src/lib.rs",
                "pub mod a;\npub mod b;\n\npub struct Hub;\n\nimpl Hub {\n    pub fn run(&self) {\n        a::step();\n    }\n}\n\npub fn start() {\n    a::step();\n}\n",
            ),
            (
                "src/a.rs",
                "pub fn step() {\n    crate::b::finish();\n}\n\npub fn dup() {}\n",
            ),
            ("src/b.rs", "pub fn finish() {}\n\npub fn dup() {}\n"),
        ] {
            fs::write(root.join(path), body).expect("write");
        }
        let (code, line) = world.line(&["index", "."]);
        assert_eq!(code, Some(0), "{line}");
        world
    }

    fn line_with(&self, bin: &str, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(bin)
            .args(args)
            .current_dir(self.tree.path())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        (
            output.status.code(),
            stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned(),
        )
    }

    fn line(&self, args: &[&str]) -> (Option<i32>, String) {
        self.line_with(BINS[0], args)
    }

    fn data(&self, args: &[&str]) -> Value {
        let (code, line) = self.line(args);
        assert_eq!(code, Some(0), "{args:?}: {line}");
        serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone()
    }

    fn store(&self) -> Store {
        Store::open(&database_path(self.data.path())).expect("store")
    }

    /// The id of the one symbol with this stable key.
    fn id(&self, key: &str) -> i64 {
        self.store()
            .symbol_by_stable_key(key)
            .expect("lookup")
            .expect("indexed")
    }

    /// Each `symbol_path` call's text item from one server rooted at the
    /// tree, checked against its `structuredContent`, and the tool list.
    fn mcp(&self, extra: &[&str], calls: &[Value]) -> (Vec<Value>, Vec<String>) {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"path\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
        );
        for (id, arguments) in (2..).zip(calls) {
            let message = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": "symbol_path", "arguments": arguments },
            });
            input.push_str(&format!("{message}\n"));
        }
        let mut child = Command::new(BINS[0])
            .arg("mcp")
            .args(extra)
            .current_dir(self.tree.path())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
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
        let replies: Vec<Value> = String::from_utf8(output.stdout)
            .expect("utf-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(replies.len(), calls.len() + 2);
        let texts = replies[2..]
            .iter()
            .map(|reply| {
                let result = &reply["result"];
                let text = result["content"][0]["text"].as_str().expect("text");
                let parsed: Value = serde_json::from_str(text).expect("envelope");
                assert_eq!(result["structuredContent"], parsed);
                assert_eq!(result["isError"], json!(parsed["ok"] == false));
                text.to_owned()
            })
            .collect();
        let tools = replies[1]["result"]["tools"]
            .as_array()
            .expect("tools")
            .clone();
        (tools, texts)
    }
}

/// `data` as the analyze crate's `path` gives it for these keys.
fn expected(world: &World, from: &str, to: &str, max_depth: u8) -> Value {
    let node = |node: &phantom_fractal_analyze::PathNode| json!({ "key": node.key, "name": node.name, "path": node.path, "line": node.line });
    let found = path(&world.store(), world.id(from), world.id(to), max_depth).expect("path");
    json!({
        "found": found.found,
        "from": node(found.start.as_ref().expect("start")),
        "hops": found.hops.iter().map(|hop| json!({
            "key": hop.key,
            "name": hop.name,
            "path": hop.path,
            "line": hop.line,
            "edge_kind": hop.edge_kind.as_str(),
            "confidence": hop.confidence,
        })).collect::<Vec<_>>(),
        "confidence": found.confidence,
        "farthest": found.farthest.as_ref().map(node),
        "max_depth": max_depth,
    })
}

const START: &str = "src/lib.rs#start#function#0";
const STEP: &str = "src/a.rs#step#function#0";
const FINISH: &str = "src/b.rs#finish#function#0";
const HUB: &str = "src/lib.rs#Hub#struct#0";

#[test]
fn paths_match_the_analyze_crate_and_name_each_hop() {
    let world = World::new();
    let db = database(world.data.path());
    for bin in BINS {
        let (code, line) = world.line_with(bin, &["path", "start", "b::finish"]);
        assert_eq!(code, Some(0), "{line}");
        assert_eq!(
            raw_keys(&line, 0),
            ["ok", "data", "meta", "error", "schema"]
        );
        let data_at = value_at(&line, "data", 0);
        assert_eq!(
            raw_keys(&line, data_at),
            [
                "found",
                "from",
                "hops",
                "confidence",
                "farthest",
                "max_depth"
            ]
        );
        let hop_at = value_at(&line, "hops", data_at) + 1;
        assert_eq!(
            raw_keys(&line, hop_at),
            ["key", "name", "path", "line", "edge_kind", "confidence"]
        );
    }
    let data = world.data(&["path", "start", "b::finish"]);
    assert_eq!(data, expected(&world, START, FINISH, 10));
    let keys: Vec<&str> = data["hops"]
        .as_array()
        .expect("hops")
        .iter()
        .map(|hop| hop["key"].as_str().expect("key"))
        .collect();
    assert_eq!(keys, [STEP, FINISH]);

    // A type reaches its method's calls through `has_member`.
    let data = world.data(&["path", "Hub", "finish"]);
    assert_eq!(data, expected(&world, HUB, FINISH, 10));
    let kinds: Vec<&str> = data["hops"]
        .as_array()
        .expect("hops")
        .iter()
        .map(|hop| hop["edge_kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["has_member", "calls", "calls"]);

    // Keys work as names do; `from == to` is zero hops.
    assert_eq!(
        world.data(&["path", START, FINISH]),
        expected(&world, START, FINISH, 10)
    );
    let data = world.data(&["path", "start", "start"]);
    assert_eq!(data["found"], true);
    assert_eq!(data["hops"], json!([]));
    assert_eq!(data["confidence"], 1.0);

    // Backwards there is no path, and the depth cut is inclusive.
    let data = world.data(&["path", "finish", "start"]);
    assert_eq!(data, expected(&world, FINISH, START, 10));
    assert_eq!(data["found"], false);
    assert_eq!(data["confidence"], Value::Null);
    assert_eq!(data["farthest"]["key"], FINISH);
    let data = world.data(&["path", "start", "finish", "--max-depth", "1"]);
    assert_eq!(data, expected(&world, START, FINISH, 1));
    assert_eq!(data["farthest"]["key"], STEP);
    assert_eq!(
        world.data(&["path", "start", "finish", "--max-depth", "2"])["found"],
        true
    );
    assert_eq!(database(world.data.path()), db, "path wrote");
}

#[test]
fn unknown_ambiguous_and_out_of_range_are_refused() {
    let world = World::new();
    let (code, line) = world.line(&["path", "dup", "start"]);
    assert_eq!(code, Some(6), "{line}");
    let envelope: Value = serde_json::from_str(&line).expect("envelope");
    assert_eq!(envelope["error"]["kind"], "ambiguous");
    assert_eq!(envelope["data"]["total"], 2);
    assert!(envelope["data"]["candidates"][0]["stable_key"].is_string());
    for args in [
        &["path", "start", "nosuch"][..],
        &["path", "nosuch", "start"],
    ] {
        let (code, line) = world.line(args);
        assert_eq!(code, Some(3), "{line}");
        assert!(line.contains("\"kind\":\"not_found\""), "{line}");
    }
    for args in [
        &["path", "start", "finish", "--max-depth", "0"][..],
        &["path", "start", "finish", "--max-depth", "21"],
        &["path", "start"],
        &["path", "start", "finish", "--max-tokens", "0"],
    ] {
        let output = Command::new(BINS[0])
            .args(args)
            .current_dir(world.tree.path())
            .env("PHANTOM_FRACTAL_DATA", world.data.path())
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    let empty = TempDir::new("path-empty");
    let output = Command::new(BINS[0])
        .args(["path", "a", "b"])
        .current_dir(empty.path())
        .env("PHANTOM_FRACTAL_DATA", world.data.path())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_path_is_never_cut_by_a_budget() {
    let world = World::new();
    let (_, full) = world.line(&["path", "Hub", "finish"]);
    let (code, line) = world.line(&["path", "Hub", "finish", "--max-tokens", "100000"]);
    assert_eq!(code, Some(0), "{line}");
    let data = &serde_json::from_str::<Value>(&line).expect("envelope")["data"];
    assert_eq!(data["tokens"]["omitted"], 0);
    let full_data = &serde_json::from_str::<Value>(&full).expect("envelope")["data"];
    assert_eq!(data["hops"], full_data["hops"]);
    let used = data["tokens"]["used"].as_u64().expect("used");
    assert_eq!(used, (line.len() as f64 / 3.5).ceil() as u64 + 1);
    // Every budget gives the whole path or `budget_too_small`, never fewer
    // hops: from 1 to past what the reply needs, both happen.
    let (mut fitted, mut small) = (0, 0);
    for max in 1..=used * 100 / 95 + 10 {
        let (code, line) = world.line(&["path", "Hub", "finish", "--max-tokens", &max.to_string()]);
        let envelope: Value = serde_json::from_str(&line).expect("envelope");
        if code == Some(0) {
            assert_eq!(envelope["data"]["hops"], full_data["hops"], "{max}");
            assert_eq!(envelope["data"]["tokens"]["omitted"], 0, "{max}");
            fitted += 1;
        } else {
            assert_eq!(code, Some(1), "{max}: {line}");
            assert!(line.contains("budget_too_small"), "{line}");
            small += 1;
        }
    }
    assert!(
        fitted >= 5 && small >= 100,
        "{fitted} fitted, {small} too small"
    );
}

#[test]
fn symbol_path_is_the_cli_line_and_keeps_to_the_roots() {
    let world = World::new();
    let pairs: Vec<(Value, Vec<&str>)> = vec![
        (
            json!({ "from": "start", "to": "finish" }),
            vec!["path", "start", "finish"],
        ),
        (
            json!({ "from": HUB, "to": FINISH }),
            vec!["path", HUB, FINISH],
        ),
        (
            json!({ "from": "start", "to": "finish", "max_depth": 1 }),
            vec!["path", "start", "finish", "--max-depth", "1"],
        ),
        (
            json!({ "from": "Hub", "to": "finish", "max_tokens": 5000 }),
            vec!["path", "Hub", "finish", "--max-tokens", "5000"],
        ),
        (
            json!({ "from": "dup", "to": "start" }),
            vec!["path", "dup", "start"],
        ),
        (
            json!({ "from": "nosuch", "to": "start" }),
            vec!["path", "nosuch", "start"],
        ),
    ];
    let calls: Vec<Value> = pairs
        .iter()
        .map(|(arguments, _)| arguments.clone())
        .collect();
    let (tools, texts) = world.mcp(&[], &calls);
    for ((arguments, args), text) in pairs.iter().zip(&texts) {
        assert_eq!(text, &world.line(args).1, "{arguments}");
    }
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    let at = names
        .iter()
        .position(|name| *name == "symbol_path")
        .expect("listed");
    assert_eq!(
        names[at - 1..=at + 1],
        ["diff_impact", "symbol_path", "repo_status"]
    );
    let schema = &tools[at]["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["from", "to"]));
    assert_eq!(
        schema["properties"]["max_depth"],
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 20,
            "default": 10,
            "description": schema["properties"]["max_depth"]["description"],
        })
    );
    assert_eq!(
        tools[at]["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": true })
    );

    let other = TempDir::new("path-other");
    let (_, texts) = world.mcp(
        &["--root", other.path().to_str().expect("utf-8")],
        &[json!({ "from": "start", "to": "finish" })],
    );
    assert_eq!(texts, [REFUSED]);
    let (_, texts) = world.mcp(
        &[],
        &[
            json!({ "from": "start" }),
            json!({ "from": "start", "to": "finish", "max_depth": 21 }),
            json!({ "from": "start", "to": "finish", "depth": 2 }),
            json!({ "from": 1, "to": "finish" }),
        ],
    );
    for text in &texts {
        assert!(
            text.starts_with("{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\""),
            "{text}"
        );
    }
}
