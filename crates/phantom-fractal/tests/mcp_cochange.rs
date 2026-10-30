//! The MCP `cochange` tool and the `index_repo` window arguments
//! (SPEC-w8 G3b), on a synthetic git root from `support`.
//!
//! Each tool result's text item must be the command line's line for the
//! same arguments, byte for byte, and its `structuredContent` must parse
//! equal to it.

mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use support::{Repo, TempDir, history};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const REFUSED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"repo is not an indexed root of this server\",\"exit\":2},\"schema\":1}";

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
/// its `structuredContent`.
fn serve(
    dir: &Path,
    data: &Path,
    extra: &[&str],
    calls: &[(&str, Value)],
) -> (Vec<Value>, Vec<String>) {
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"cochange\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
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
fn the_tool_sits_after_repo_map_with_the_cli_ranges() {
    let repo = Repo::new("schema", &history());
    let data = TempDir::new("data");
    let (tools, _) = serve(&repo.root, data.path(), &["--allow-index"], &[]);
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names[6..],
        [
            "repo_map",
            "cochange",
            "diff_impact",
            "symbol_path",
            "repo_status",
            "repos",
            "index_repo"
        ]
    );
    let tool = &tools[7];
    assert_eq!(
        tool["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": true })
    );
    let schema = &tool["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["file"]));
    let properties = &schema["properties"];
    let described = |name: &str, mut want: Value| {
        want["description"] = properties[name]["description"].clone();
        assert!(want["description"].is_string(), "{name}");
        assert_eq!(properties[name], want, "{name}");
    };
    described("file", json!({ "type": "string" }));
    described(
        "min_lift",
        json!({ "type": "number", "minimum": 0, "default": 1.5 }),
    );
    described(
        "limit",
        json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }),
    );
    described("repo", json!({ "type": "string" }));
    described(
        "max_tokens",
        json!({ "type": "integer", "minimum": 1, "maximum": 100_000 }),
    );
    let index_repo = tools
        .iter()
        .find(|tool| tool["name"] == "index_repo")
        .expect("index_repo");
    let index = &index_repo["inputSchema"]["properties"];
    assert_eq!(index["cochange_days"]["minimum"], 1);
    assert_eq!(index["cochange_days"]["maximum"], 3650);
    assert_eq!(index["cochange_days"]["default"], 365);
    assert_eq!(index["cochange_commits"]["minimum"], 1);
    assert_eq!(index["cochange_commits"]["maximum"], 50_000);
    assert_eq!(index["cochange_commits"]["default"], 5000);
    assert_eq!(index_repo["inputSchema"]["required"], json!(["path"]));
}

#[test]
fn cochange_matches_the_cli_and_keeps_to_the_roots() {
    let repo = Repo::new("parity", &history());
    let data = TempDir::new("data");
    let line = cli(&repo.root, data.path(), &["index", "."]);
    assert!(line.starts_with("{\"ok\":true"), "{line}");
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (json!({ "file": "src/a.rs" }), vec!["cochange", "src/a.rs"]),
        (
            json!({ "file": "./src/a.rs" }),
            vec!["cochange", "./src/a.rs"],
        ),
        (
            json!({ "file": "src/a.rs", "min_lift": 0, "limit": 2 }),
            vec!["cochange", "src/a.rs", "--min-lift", "0", "--limit", "2"],
        ),
        (
            json!({ "file": "src/a.rs", "min_lift": 2.5, "limit": 100 }),
            vec![
                "cochange",
                "src/a.rs",
                "--min-lift",
                "2.5",
                "--limit",
                "100",
            ],
        ),
        (
            json!({ "file": "src/a.rs", "min_lift": 0, "max_tokens": 120 }),
            vec![
                "cochange",
                "src/a.rs",
                "--min-lift",
                "0",
                "--max-tokens",
                "120",
            ],
        ),
        (
            json!({ "file": "src/a.rs", "max_tokens": 1 }),
            vec!["cochange", "src/a.rs", "--max-tokens", "1"],
        ),
        (
            json!({ "file": "src/lib.rs" }),
            vec!["cochange", "src/lib.rs"],
        ),
        (
            json!({ "file": "src/nope.rs" }),
            vec!["cochange", "src/nope.rs"],
        ),
        (
            json!({ "file": "src/a.rs", "repo": "." }),
            vec!["cochange", "src/a.rs", "--repo", "."],
        ),
    ];
    let calls: Vec<(&str, Value)> = cases
        .iter()
        .map(|(arguments, _)| ("cochange", arguments.clone()))
        .collect();
    let (_, texts) = serve(&repo.root, data.path(), &[], &calls);
    let mut kinds = Vec::new();
    for (text, (arguments, args)) in texts.iter().zip(&cases) {
        assert_eq!(*text, cli(&repo.root, data.path(), args), "{arguments}");
        let envelope: Value = serde_json::from_str(text).expect("envelope");
        kinds.push(
            envelope["error"]["kind"]
                .as_str()
                .unwrap_or("ok")
                .to_owned(),
        );
    }
    assert_eq!(
        kinds,
        [
            "ok",
            "ok",
            "ok",
            "ok",
            "ok",
            "usage",
            "ok",
            "not_found",
            "ok"
        ]
    );
    // Not a root, or no index: the one failure.
    let (_, texts) = serve(
        &repo.root,
        data.path(),
        &[],
        &[
            ("cochange", json!({ "file": "src/a.rs", "repo": "src" })),
            ("cochange", json!({ "file": "src/a.rs", "repo": "missing" })),
            ("cochange", json!({ "file": "src/a.rs", "repo": ".." })),
        ],
    );
    assert_eq!(texts, vec![REFUSED; 3]);
}

#[test]
fn bad_cochange_arguments_are_usage() {
    let repo = Repo::new("arguments", &history());
    let data = TempDir::new("data");
    cli(&repo.root, data.path(), &["index", "."]);
    let cases = [
        (json!({}), "file is required"),
        (json!({ "file": 1 }), "file must be a string"),
        (
            json!({ "file": "src/a.rs", "min_lift": -1 }),
            "min_lift must be a number at least 0",
        ),
        (
            json!({ "file": "src/a.rs", "min_lift": "2" }),
            "min_lift must be a number at least 0",
        ),
        (
            json!({ "file": "src/a.rs", "limit": 0 }),
            "limit must be an integer from 1 to 100",
        ),
        (
            json!({ "file": "src/a.rs", "limit": 101 }),
            "limit must be an integer from 1 to 100",
        ),
        (
            json!({ "file": "src/a.rs", "max_tokens": 0 }),
            "max_tokens must be an integer from 1 to 100000",
        ),
        (
            json!({ "file": "src/a.rs", "symbol": "a" }),
            "unknown argument; this tool takes file, min_lift, limit, repo, max_tokens",
        ),
        (
            json!({ "file": "src/a.rs", "min_lift": -1, "repo": "missing" }),
            "min_lift must be a number at least 0",
        ),
    ];
    let calls: Vec<(&str, Value)> = cases
        .iter()
        .map(|(arguments, _)| ("cochange", arguments.clone()))
        .collect();
    let (_, texts) = serve(&repo.root, data.path(), &[], &calls);
    for (text, (arguments, message)) in texts.iter().zip(&cases) {
        let envelope: Value = serde_json::from_str(text).expect("envelope");
        assert_eq!(
            envelope["error"],
            json!({ "kind": "usage", "message": message, "exit": 1 }),
            "{arguments}"
        );
    }
}

#[test]
fn index_repo_takes_the_window_as_the_cli_does() {
    let mcp_repo = Repo::new("index-mcp", &history());
    let cli_repo = Repo::new("index-cli", &history());
    let (mcp_data, cli_data) = (TempDir::new("mcp-data"), TempDir::new("cli-data"));
    let (_, texts) = serve(
        &mcp_repo.root,
        mcp_data.path(),
        &["--allow-index"],
        &[
            ("index_repo", json!({ "path": "." })),
            ("index_repo", json!({ "path": ".", "cochange_days": 10 })),
            (
                "index_repo",
                json!({ "path": ".", "cochange_days": 10, "cochange_commits": 3 }),
            ),
            ("index_repo", json!({ "path": ".", "cochange_days": 0 })),
            (
                "index_repo",
                json!({ "path": ".", "cochange_commits": 50_001 }),
            ),
            ("index_repo", json!({ "path": "src" })),
        ],
    );
    let want = [
        cli(&cli_repo.root, cli_data.path(), &["index", "."]),
        cli(
            &cli_repo.root,
            cli_data.path(),
            &["index", ".", "--cochange-days", "10"],
        ),
        cli(
            &cli_repo.root,
            cli_data.path(),
            &[
                "index",
                ".",
                "--cochange-days",
                "10",
                "--cochange-commits",
                "3",
            ],
        ),
    ];
    // The two repositories are built alike but are not the same commits:
    // only HEAD differs.
    let head = |repo: &Repo| repo.rev("HEAD");
    for (text, line) in texts[..3].iter().zip(&want) {
        assert_eq!(
            *text,
            line.replace(&head(&cli_repo), &head(&mcp_repo)),
            "index_repo"
        );
    }
    let usage = |text: &str| -> Value {
        serde_json::from_str::<Value>(text).expect("envelope")["error"]["message"].clone()
    };
    assert_eq!(
        usage(&texts[3]),
        "cochange_days must be an integer from 1 to 3650"
    );
    assert_eq!(
        usage(&texts[4]),
        "cochange_commits must be an integer from 1 to 50000"
    );
    assert_eq!(texts[5], REFUSED, "index_repo indexes only a root");
}
