//! The bounds on MCP arguments (SPEC-w10 G0b): `maxLength` on every string
//! argument, checked by the server before any store read, and the
//! `min_lift` maximum the command line shares.

mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use support::{Repo, TempDir, history};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

/// The tools that take `repo`.
const REPO_TOOLS: [&str; 11] = [
    "find_symbols",
    "show_symbol",
    "callers",
    "callees",
    "impact",
    "top_ranked",
    "repo_map",
    "cochange",
    "diff_impact",
    "symbol_path",
    "repo_status",
];

/// Arguments that make a call to `tool` well formed apart from the one
/// under test.
fn base(tool: &str) -> Value {
    match tool {
        "find_symbols" => json!({ "query": "top" }),
        "show_symbol" | "callers" | "callees" | "impact" => json!({ "symbol": "top" }),
        "repo_map" => json!({ "budget": 100 }),
        "cochange" => json!({ "file": "src/a.rs" }),
        "symbol_path" => json!({ "from": "top", "to": "top" }),
        "index_repo" => json!({ "path": "." }),
        _ => json!({}),
    }
}

/// Every bounded argument: its tool, its name, and its bound.
fn bounds() -> Vec<(&'static str, &'static str, usize)> {
    let mut all = vec![
        ("find_symbols", "query", 1024),
        ("show_symbol", "symbol", 1024),
        ("show_symbol", "id", 1024),
        ("callers", "symbol", 1024),
        ("callees", "symbol", 1024),
        ("impact", "symbol", 1024),
        ("top_ranked", "kind", 1024),
        ("top_ranked", "focus", 4096),
        ("repo_map", "focus", 4096),
        ("repo_map", "detail", 1024),
        ("cochange", "file", 4096),
        ("diff_impact", "against", 256),
        ("symbol_path", "from", 1024),
        ("symbol_path", "to", 1024),
        ("index_repo", "path", 4096),
    ];
    all.extend(REPO_TOOLS.iter().map(|tool| (*tool, "repo", 4096)));
    all.sort();
    all
}

/// `count` Unicode scalar values: ASCII, or a mix of two-, three- and
/// four-byte characters.
fn text(count: usize, wide: bool) -> String {
    if wide {
        ['é', '字', '😀'].iter().cycle().take(count).collect()
    } else {
        "a".repeat(count)
    }
}

/// A call to `tool` with `name` set to a text of `count` characters.
fn call(tool: &'static str, name: &str, count: usize, wide: bool) -> (&'static str, Value) {
    let mut arguments = base(tool);
    let value = text(count, wide);
    arguments[name] = if name == "focus" {
        json!([value])
    } else {
        json!(value)
    };
    // `show_symbol` takes exactly one of `symbol` and `id`.
    if tool == "show_symbol" && name == "id" {
        arguments
            .as_object_mut()
            .expect("arguments")
            .remove("symbol");
    }
    (tool, arguments)
}

/// The `tools/list` result, then each call's text item, checked against
/// its `structuredContent`.
fn serve(dir: &Path, data: &Path, calls: &[(&str, Value)]) -> (Vec<Value>, Vec<String>) {
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"bounds\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
    );
    for (id, (name, arguments)) in (2..).zip(calls) {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        input.push_str(&format!("{message}\n"));
    }
    let mut child = Command::new(BIN)
        .args(["mcp", "--allow-index"])
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    // Written from a thread: the replies may outgrow the pipe before the
    // last request is sent.
    let mut stdin = child.stdin.take().expect("stdin");
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output().expect("wait");
    writer.join().expect("writer").expect("write stdin");
    assert_eq!(output.status.code(), Some(0));
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON line"))
        .collect();
    assert_eq!(replies.len(), calls.len() + 2);
    let tools = replies[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone();
    let texts = replies[2..]
        .iter()
        .map(|reply| {
            let result = &reply["result"];
            let text = result["content"][0]["text"].as_str().expect("text");
            let parsed: Value = serde_json::from_str(text).expect("envelope");
            assert_eq!(result["structuredContent"], parsed);
            text.to_owned()
        })
        .collect();
    (tools, texts)
}

/// The command line's exit code, stdout and stderr.
fn cli(dir: &Path, data: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
        .output()
        .expect("run cli");
    (
        output.status.code(),
        String::from_utf8(output.stdout).expect("utf-8"),
        String::from_utf8(output.stderr).expect("utf-8"),
    )
}

/// The entries under `dir`.
fn entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).expect("read data").count()
}

#[test]
fn tools_list_bounds_every_string_argument() {
    let repo = Repo::new("bounds-list", &history());
    let data = TempDir::new("data");
    let (tools, _) = serve(&repo.root, data.path(), &[]);
    let mut listed = Vec::new();
    for tool in &tools {
        let name = tool["name"].as_str().expect("name");
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .expect("properties");
        for (argument, property) in properties {
            let text = if property["type"] == "array" {
                &property["items"]
            } else {
                property
            };
            if text["type"] == "string" {
                let bound = text["maxLength"].as_u64().map(|bound| bound as usize);
                listed.push((name, argument.as_str(), bound));
            } else {
                assert!(text.get("maxLength").is_none(), "{name}.{argument}");
            }
        }
    }
    listed.sort();
    let want: Vec<_> = bounds()
        .into_iter()
        .map(|(tool, name, bound)| (tool, name, Some(bound)))
        .collect();
    assert_eq!(listed, want);
    let cochange = tools
        .iter()
        .find(|tool| tool["name"] == "cochange")
        .expect("cochange");
    let min_lift = &cochange["inputSchema"]["properties"]["min_lift"];
    assert_eq!(min_lift["minimum"], 0);
    assert_eq!(min_lift["maximum"], 1000);
}

#[test]
fn each_bound_takes_n_characters_and_refuses_n_plus_one_unechoed() {
    let repo = Repo::new("bounds-calls", &history());
    let indexed = TempDir::new("indexed");
    let (code, _, _) = cli(&repo.root, indexed.path(), &["index", "."]);
    assert_eq!(code, Some(0));
    let empty = TempDir::new("empty");
    let cases: Vec<(&str, &str, usize, bool)> = bounds()
        .into_iter()
        .flat_map(|(tool, name, bound)| [false, true].map(|wide| (tool, name, bound, wide)))
        .collect();

    // N characters pass the check. The text is too wide for a bound in
    // bytes, so this counts characters.
    let at: Vec<_> = cases
        .iter()
        .map(|(tool, name, bound, wide)| call(tool, name, *bound, *wide))
        .collect();
    let (_, texts) = serve(&repo.root, indexed.path(), &at);
    for (text, (tool, name, bound, wide)) in texts.iter().zip(&cases) {
        assert!(
            !text.contains("is longer than"),
            "{tool}.{name} {bound} {wide}: {text}"
        );
    }
    let find = cases
        .iter()
        .position(|case| case.0 == "find_symbols" && case.1 == "query")
        .expect("find");
    assert!(texts[find].starts_with("{\"ok\":true"), "{}", texts[find]);

    // N + 1 characters are a usage failure that names the argument and
    // its bound, never the value. With no index at all, the failure is
    // still this one, and nothing is written: the check runs before the
    // store is opened.
    let over: Vec<_> = cases
        .iter()
        .map(|(tool, name, bound, wide)| call(tool, name, bound + 1, *wide))
        .collect();
    let (_, texts) = serve(&repo.root, empty.path(), &over);
    for (text, (tool, name, bound, wide)) in texts.iter().zip(&cases) {
        let envelope: Value = serde_json::from_str(text).expect("envelope");
        let message = format!("{name} is longer than {bound} characters");
        assert_eq!(
            envelope,
            json!({
                "ok": false,
                "data": null,
                "error": { "kind": "usage", "message": message, "exit": 1 },
                "schema": 1,
            }),
            "{tool}.{name} {wide}"
        );
        assert!(!text.contains(&text_prefix(*wide)), "{tool}.{name}: {text}");
    }
    assert_eq!(entries(empty.path()), 0);
}

/// The first eight characters of every test text.
fn text_prefix(wide: bool) -> String {
    text(8, wide)
}

#[test]
fn min_lift_takes_1000_and_refuses_more_in_the_cli_and_mcp() {
    let repo = Repo::new("bounds-lift", &history());
    let data = TempDir::new("data");
    let (code, _, _) = cli(&repo.root, data.path(), &["index", "."]);
    assert_eq!(code, Some(0));
    let message = "min_lift must be a number from 0 to 1000";

    let (code, stdout, _) = cli(
        &repo.root,
        data.path(),
        &["cochange", "src/a.rs", "--min-lift", "1000"],
    );
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("{\"ok\":true"), "{stdout}");
    for over in ["1000.000001", "1001", "-0.000001"] {
        let flag = format!("--min-lift={over}");
        let (code, stdout, stderr) = cli(&repo.root, data.path(), &["cochange", "src/a.rs", &flag]);
        assert_eq!((code, stdout.as_str()), (Some(1), ""), "{over}");
        assert!(stderr.contains(message), "{over}: {stderr}");
    }

    let calls = [
        ("cochange", json!({ "file": "src/a.rs", "min_lift": 1000 })),
        (
            "cochange",
            json!({ "file": "src/a.rs", "min_lift": 1000.0 }),
        ),
        (
            "cochange",
            json!({ "file": "src/a.rs", "min_lift": 1000.000001 }),
        ),
        ("cochange", json!({ "file": "src/a.rs", "min_lift": 1001 })),
    ];
    let (_, texts) = serve(&repo.root, data.path(), &calls);
    let (_, cli_line, _) = cli(
        &repo.root,
        data.path(),
        &["cochange", "src/a.rs", "--min-lift", "1000"],
    );
    assert_eq!(texts[0], cli_line.trim_end());
    assert_eq!(texts[1], cli_line.trim_end());
    for text in &texts[2..] {
        let envelope: Value = serde_json::from_str(text).expect("envelope");
        assert_eq!(
            envelope["error"],
            json!({ "kind": "usage", "message": message, "exit": 1 }),
        );
    }
}
