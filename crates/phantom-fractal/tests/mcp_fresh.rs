//! Freshness over MCP (SPEC-w9 G0c): `meta` in every tool result as the
//! command line prints it, the `repo_status` tool, and `index_repo` under
//! another writer's lock.
//!
//! Each tool result's text item must be the command line's line for the
//! same arguments, byte for byte, and its `structuredContent` must parse
//! equal to it.

mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use phantom_fractal_store::WriterLock;
use serde_json::{Value, json};

use support::{Repo, TempDir, database_path, history, raw_keys};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const REFUSED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"repo is not an indexed root of this server\",\"exit\":2},\"schema\":1}";

const LOCK_HELD: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"lock_held\",\"message\":\"store: another writer holds the database lock\",\"exit\":4},\"schema\":1}";

/// The command line's stdout line, newline removed.
fn cli(dir: &Path, data: &Path, args: &[&str]) -> String {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
        .output()
        .expect("run cli");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned()
}

/// The `tools/list` result, then each call's text item, checked against
/// its `structuredContent` and `isError`.
fn serve(
    dir: &Path,
    data: &Path,
    extra: &[&str],
    calls: &[(&str, Value)],
) -> (Vec<Value>, Vec<String>) {
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"fresh\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
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
        .arg("mcp")
        .args(extra)
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
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
    let tools = replies[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone();
    let texts = replies[2..]
        .iter()
        .map(|reply| {
            let result = &reply["result"];
            let content = result["content"].as_array().expect("content");
            assert_eq!(content.len(), 1, "{reply}");
            let text = content[0]["text"].as_str().expect("text");
            let parsed: Value = serde_json::from_str(text).expect("envelope");
            assert_eq!(result["structuredContent"], parsed);
            assert_eq!(result["isError"], json!(parsed["ok"] == false));
            text.to_owned()
        })
        .collect();
    (tools, texts)
}

#[test]
fn every_tool_reply_carries_the_cli_line_with_its_meta() {
    let repo = Repo::new("mcp-fresh", &history());
    let data = TempDir::new("data");
    let root = repo.root.to_str().expect("utf-8 root");
    assert_eq!(
        raw_keys(&cli(&repo.root, data.path(), &["index", "."]), 0),
        ["ok", "data", "meta", "error", "schema"]
    );
    // A stale tree: the replies must say so, and still answer.
    std::fs::write(repo.root.join("src/new.rs"), "pub fn fresh() {}\n").expect("add");
    let pairs: [(&str, Value, &[&str]); 9] = [
        ("find_symbols", json!({ "query": "fa" }), &["find", "fa"]),
        (
            "show_symbol",
            json!({ "symbol": "fa", "max_tokens": 400 }),
            &["show", "fa", "--max-tokens", "400"],
        ),
        ("callers", json!({ "symbol": "fa" }), &["callers", "fa"]),
        ("callees", json!({ "symbol": "top" }), &["callees", "top"]),
        ("impact", json!({ "symbol": "fa" }), &["impact", "fa"]),
        (
            "top_ranked",
            json!({ "kind": "files" }),
            &["rank", "--files"],
        ),
        (
            "repo_map",
            json!({ "budget": 300 }),
            &["map", "--budget", "300"],
        ),
        (
            "cochange",
            json!({ "file": "src/a.rs", "repo": root }),
            &["cochange", "src/a.rs"],
        ),
        ("repo_status", json!({}), &["status"]),
    ];
    let calls: Vec<(&str, Value)> = pairs
        .iter()
        .map(|(name, arguments, _)| (*name, arguments.clone()))
        .collect();
    let (_, texts) = serve(&repo.root, data.path(), &[], &calls);
    for ((name, _, args), text) in pairs.iter().zip(&texts) {
        assert_eq!(text, &cli(&repo.root, data.path(), args), "{name}");
        assert_eq!(
            raw_keys(text, 0),
            ["ok", "data", "meta", "error", "schema"],
            "{name}"
        );
        assert!(text.contains("\"meta\":{\"stale\":true,"), "{name}: {text}");
    }
    let status: Value = serde_json::from_str(&texts[8]).expect("status");
    assert_eq!(status["data"]["changes"]["added"], 1);
    assert_eq!(status["data"]["lock"], "free");
}

#[test]
fn repo_status_is_read_only_and_keeps_to_the_roots() {
    let repo = Repo::new("mcp-status", &history());
    let data = TempDir::new("data");
    let other = TempDir::new("other");
    let (tools, texts) = serve(
        &repo.root,
        data.path(),
        &["--root", other.path().to_str().expect("utf-8")],
        &[
            // No index under the only root, a path that is not a root, and
            // a missing path all fail alike.
            ("repo_status", json!({})),
            (
                "repo_status",
                json!({ "repo": repo.root.to_str().expect("utf-8") }),
            ),
            ("repo_status", json!({ "repo": "/no/such/path" })),
            ("repo_status", json!({ "path": "." })),
            ("repo_status", json!({ "repo": 5 })),
        ],
    );
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "repo_status")
        .expect("repo_status");
    assert_eq!(
        tool["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": true })
    );
    let schema = &tool["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!([]));
    assert_eq!(
        schema["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .collect::<Vec<_>>(),
        ["repo"]
    );
    assert_eq!(texts[0], REFUSED);
    assert_eq!(texts[1], REFUSED);
    assert_eq!(texts[2], REFUSED);
    assert_eq!(
        texts[3],
        "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"unknown argument; this tool takes repo\",\"exit\":1},\"schema\":1}"
    );
    assert_eq!(
        texts[4],
        "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"repo must be a string\",\"exit\":1},\"schema\":1}"
    );
}

#[test]
fn index_repo_under_another_writers_lock_is_lock_held() {
    let repo = Repo::new("mcp-lock", &history());
    let data = TempDir::new("data");
    let root = repo.root.to_str().expect("utf-8 root");
    cli(&repo.root, data.path(), &["index", "."]);
    let database = database_path(data.path());
    let guard = WriterLock::acquire(&database).expect("take the lock");
    let (_, texts) = serve(
        &repo.root,
        data.path(),
        &["--allow-index"],
        &[
            ("index_repo", json!({ "path": root })),
            ("repo_status", json!({})),
        ],
    );
    assert_eq!(texts[0], LOCK_HELD);
    let status: Value = serde_json::from_str(&texts[1]).expect("status");
    assert_eq!(status["data"]["lock"], "held");
    drop(guard);
    let (_, texts) = serve(
        &repo.root,
        data.path(),
        &["--allow-index"],
        &[("index_repo", json!({ "path": root }))],
    );
    assert_eq!(texts[0], cli(&repo.root, data.path(), &["index", "."]));
    assert!(
        texts[0].contains("\"meta\":{\"stale\":false,"),
        "{}",
        texts[0]
    );
}
