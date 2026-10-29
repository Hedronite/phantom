//! `mcp` driven by a scripted client: the lifecycle, every protocol error,
//! the tool list, bad arguments, and stdout carrying only JSON-RPC lines
//! (SPEC-w6 G2a, G2b, G2c).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// 1 MiB, the longest line the server reads.
const MAX_LINE: usize = 1 << 20;

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-mcp-{label}-{}-{}",
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

/// What one server run printed.
struct Run {
    lines: Vec<String>,
    stderr: Vec<u8>,
    code: Option<i32>,
}

impl Run {
    /// Every stdout line, parsed. Each must be a JSON-RPC 2.0 response.
    fn replies(&self) -> Vec<Value> {
        self.lines
            .iter()
            .map(|line| {
                let reply: Value = serde_json::from_str(line).expect("stdout line is JSON");
                assert_eq!(reply["jsonrpc"], "2.0", "{line}");
                assert!(reply.get("id").is_some(), "{line}");
                assert!(
                    reply.get("result").is_some() != reply.get("error").is_some(),
                    "{line}"
                );
                reply
            })
            .collect()
    }
}

/// Runs `bin mcp [extra]` in `cwd` with the data directory at `data`, feeds
/// it `input`, and closes stdin.
fn serve(bin: &str, extra: &[&str], cwd: &Path, data: &Path, input: &[u8]) -> Run {
    let mut child = Command::new(bin)
        .arg("mcp")
        .args(extra)
        .current_dir(cwd)
        .env("PHANTOM_FRACTAL_DATA", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let mut stdin = child.stdin.take().expect("stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        stdin.write_all(&input).expect("write stdin");
    });
    let output = child.wait_with_output().expect("wait");
    writer.join().expect("writer");
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    assert!(stdout.is_empty() || stdout.ends_with('\n'), "{stdout}");
    Run {
        lines: stdout.lines().map(str::to_owned).collect(),
        stderr: output.stderr,
        code: output.status.code(),
    }
}

/// Each message on its own line.
fn transcript(messages: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for message in messages {
        out.extend(serde_json::to_vec(message).expect("encode"));
        out.push(b'\n');
    }
    out
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn initialize(id: i64, version: &str) -> Value {
    request(
        id,
        "initialize",
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": { "name": "scripted", "version": "0" },
        }),
    )
}

fn initialized() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

fn call(id: i64, name: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

fn error_code(reply: &Value) -> i64 {
    reply["error"]["code"].as_i64().expect("error code")
}

/// A server run in an empty directory with an empty data directory, which
/// must exit 0 with nothing on stderr.
fn session(extra: &[&str], input: &[u8]) -> Vec<Value> {
    let cwd = TempDir::new("cwd");
    let data = TempDir::new("data");
    let mut replies = Vec::new();
    for bin in BINS {
        let run = serve(bin, extra, cwd.path(), data.path(), input);
        assert_eq!(run.code, Some(0), "{bin}");
        assert!(
            run.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let got = run.replies();
        if bin == BINS[0] {
            replies = got;
        } else {
            assert_eq!(got, replies, "both binary names answer the same");
        }
    }
    replies
}

#[test]
fn before_initialize_only_ping_and_initialize_are_served() {
    let replies = session(
        &[],
        &transcript(&[
            request(1, "tools/list", json!({})),
            call(2, "find_symbols", json!({ "query": "x" })),
            request(3, "server/discover", json!({})),
            request(4, "no/such/method", json!({})),
            request(5, "ping", json!({})),
            initialized(),
            request(6, "tools/list", json!({})),
            initialize(7, "2025-11-25"),
            request(8, "tools/list", json!({})),
        ]),
    );
    assert_eq!(replies.len(), 8, "the notification gets no reply");
    for reply in &replies[..4] {
        assert_eq!(error_code(reply), -32002, "{reply}");
    }
    assert_eq!(
        replies[4],
        json!({ "jsonrpc": "2.0", "id": 5, "result": {} })
    );
    assert_eq!(error_code(&replies[5]), -32002);
    assert_eq!(replies[6]["id"], 7);
    assert!(replies[7]["result"]["tools"].is_array());
}

#[test]
fn initialize_echoes_a_supported_version_and_otherwise_answers_the_newest() {
    let cases = [
        ("2025-11-25", "2025-11-25"),
        ("2025-06-18", "2025-06-18"),
        ("2025-03-26", "2025-11-25"),
        ("2024-11-05", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
        ("1900-01-01", "2025-11-25"),
    ];
    let mut messages: Vec<Value> = cases
        .iter()
        .zip(1..)
        .map(|((asked, _), id)| initialize(id, asked))
        .collect();
    messages.push(request(99, "initialize", json!({})));
    let replies = session(&[], &transcript(&messages));
    for (reply, (asked, answered)) in replies.iter().zip(&cases) {
        assert_eq!(reply["result"]["protocolVersion"], *answered, "{asked}");
    }
    let last = &replies[cases.len()]["result"];
    assert_eq!(last["protocolVersion"], "2025-11-25", "no version asked");
    for reply in &replies {
        let result = &reply["result"];
        assert_eq!(
            result["serverInfo"],
            json!({ "name": "phantom-fractal", "version": "0.0.0" })
        );
        assert_eq!(
            result["capabilities"],
            json!({ "tools": { "listChanged": false } })
        );
        let instructions = result["instructions"].as_str().expect("instructions");
        assert!(!instructions.is_empty() && !instructions.contains('\n'));
        assert!(instructions.contains("untrusted"));
    }
}

#[test]
fn bad_lines_get_errors_and_the_server_keeps_serving() {
    // A ping padded to exactly 1 MiB is read; one byte more is not.
    let ping = |id: i64, pad: usize| {
        let head = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","params":{{"pad":""#);
        let tail = r#""}}"#;
        let mut line = head.into_bytes();
        line.resize(pad - tail.len(), b'x');
        line.extend_from_slice(tail.as_bytes());
        assert_eq!(line.len(), pad);
        line.push(b'\n');
        line
    };
    let mut input = transcript(&[initialize(1, "2025-11-25"), initialized()]);
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\"\n");
    input.extend_from_slice(b"not json at all\n");
    input.extend_from_slice(b"\xff\xfe\n");
    input.extend_from_slice(b"[{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}]\n");
    input.extend_from_slice(b"[]\n");
    input.extend_from_slice(b"42\n");
    input.extend_from_slice(b"\n   \n");
    input.extend(ping(4, MAX_LINE));
    input.extend(ping(5, MAX_LINE + 1));
    input.extend(transcript(&[
        json!({ "id": 6, "method": "ping" }),
        json!({ "jsonrpc": "1.0", "id": 7, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }),
        json!({ "jsonrpc": "2.0", "id": 8, "method": 5 }),
        json!({ "jsonrpc": "2.0", "id": 9, "method": "ping", "params": [1] }),
        json!({ "jsonrpc": "2.0", "id": 10, "result": {} }),
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": {} }),
        request(11, "resources/list", json!({})),
        request(12, "ping", json!({})),
    ]));
    // The last line has no newline and is still served.
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":13,\"method\":\"ping\"}");
    let replies = session(&[], &input);
    let got: Vec<(Value, Value)> = replies[1..]
        .iter()
        .map(|reply| {
            let outcome = reply
                .get("error")
                .map_or_else(|| reply["result"].clone(), |error| error["code"].clone());
            (reply["id"].clone(), outcome)
        })
        .collect();
    let want: Vec<(Value, Value)> = [
        (json!(null), json!(-32700)),
        (json!(null), json!(-32700)),
        (json!(null), json!(-32700)),
        (json!(null), json!(-32600)),
        (json!(null), json!(-32600)),
        (json!(null), json!(-32600)),
        (json!(4), json!({})),
        (json!(null), json!(-32600)),
        (json!(6), json!(-32600)),
        (json!(7), json!(-32600)),
        (json!(null), json!(-32600)),
        (json!(8), json!(-32600)),
        (json!(9), json!(-32602)),
        (json!(11), json!(-32601)),
        (json!(12), json!({})),
        (json!(13), json!({})),
    ]
    .into();
    assert_eq!(got, want);
    assert_eq!(replies[1]["error"]["message"], "parse error");
    assert_eq!(replies[8]["error"]["message"], "message is over 1 MiB");
}

#[test]
fn eof_at_once_exits_zero_without_a_byte() {
    let cwd = TempDir::new("cwd");
    let data = TempDir::new("data");
    for bin in BINS {
        let run = serve(bin, &[], cwd.path(), data.path(), b"");
        assert_eq!(run.code, Some(0));
        assert!(run.lines.is_empty() && run.stderr.is_empty());
    }
}

fn tools(extra: &[&str]) -> Vec<Value> {
    let replies = session(
        extra,
        &transcript(&[
            initialize(1, "2025-11-25"),
            request(2, "tools/list", json!({})),
        ]),
    );
    replies[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone()
}

fn names(tools: &[Value]) -> Vec<&str> {
    tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect()
}

#[test]
fn tools_list_matches_the_spec_table() {
    let read = tools(&[]);
    assert_eq!(
        names(&read),
        [
            "find_symbols",
            "show_symbol",
            "callers",
            "callees",
            "impact"
        ]
    );
    let all = tools(&["--allow-index"]);
    assert_eq!(
        names(&all),
        [
            "find_symbols",
            "show_symbol",
            "callers",
            "callees",
            "impact",
            "index_repo"
        ]
    );
    assert_eq!(all[..5], read[..]);
    let properties = |tool: &Value| -> Vec<String> {
        let mut keys: Vec<String> = tool["inputSchema"]["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    let want: [(&str, &[&str], &[&str]); 6] = [
        ("find_symbols", &["query", "repo"], &["query"]),
        ("show_symbol", &["id", "repo", "source", "symbol"], &[]),
        ("callers", &["repo", "symbol"], &["symbol"]),
        ("callees", &["repo", "symbol"], &["symbol"]),
        (
            "impact",
            &[
                "depth",
                "include_tests",
                "min_confidence",
                "repo",
                "summary",
                "symbol",
            ],
            &["symbol"],
        ),
        ("index_repo", &["path"], &["path"]),
    ];
    for (tool, (name, keys, required)) in all.iter().zip(want) {
        assert_eq!(tool["name"], name);
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        assert_eq!(properties(tool), keys, "{name}");
        assert_eq!(schema["required"], json!(required), "{name}");
        assert!(
            !tool["description"]
                .as_str()
                .expect("description")
                .is_empty()
        );
        let annotations = &tool["annotations"];
        if name == "index_repo" {
            assert_eq!(
                *annotations,
                json!({ "readOnlyHint": false, "destructiveHint": false })
            );
        } else {
            assert_eq!(
                *annotations,
                json!({ "readOnlyHint": true, "idempotentHint": true })
            );
        }
    }
    let impact = &all[4]["inputSchema"]["properties"];
    assert_eq!(
        impact["depth"],
        json!({
            "type": "integer", "minimum": 1, "maximum": 5, "default": 3,
            "description": impact["depth"]["description"],
        })
    );
    assert_eq!(impact["min_confidence"]["type"], "number");
    assert_eq!(impact["min_confidence"]["exclusiveMinimum"], 0);
    assert_eq!(impact["min_confidence"]["maximum"], 1);
    assert_eq!(impact["min_confidence"]["default"], 0.2);
    assert_eq!(impact["include_tests"]["default"], true);
    assert_eq!(impact["summary"]["default"], false);
    assert_eq!(
        all[1]["inputSchema"]["properties"]["source"]["default"],
        false
    );
}

/// The `structuredContent` of one tool result, checked against its text
/// item and `isError`.
fn envelope(reply: &Value) -> &Value {
    let result = &reply["result"];
    let content = result["content"].as_array().expect("content");
    assert_eq!(content.len(), 1, "{reply}");
    assert_eq!(content[0]["type"], "text");
    let text = content[0]["text"].as_str().expect("text");
    assert!(!text.contains('\n'));
    let parsed: Value = serde_json::from_str(text).expect("text is the envelope");
    let structured = &result["structuredContent"];
    assert_eq!(&parsed, structured);
    assert_eq!(
        result["isError"],
        json!(structured["ok"] == false),
        "{reply}"
    );
    structured
}

#[test]
fn unknown_tools_and_bad_arguments() {
    let replies = session(
        &[],
        &transcript(&[
            initialize(1, "2025-11-25"),
            call(2, "no_such_tool", json!({})),
            call(3, "index_repo", json!({ "path": "." })),
            request(4, "tools/call", json!({ "arguments": {} })),
            request(
                5,
                "tools/call",
                json!({ "name": "find_symbols", "arguments": [] }),
            ),
            call(6, "find_symbols", json!({})),
            call(7, "find_symbols", json!({ "query": 5 })),
            call(8, "find_symbols", json!({ "query": "x", "extra": true })),
            call(9, "show_symbol", json!({})),
            call(10, "show_symbol", json!({ "symbol": "a", "id": "b" })),
            call(11, "show_symbol", json!({ "symbol": "a", "source": "yes" })),
            call(12, "callers", json!({ "symbol": null })),
            call(13, "callees", json!({ "repo": "." })),
            call(14, "impact", json!({ "symbol": "a", "depth": 0 })),
            call(15, "impact", json!({ "symbol": "a", "depth": 6 })),
            call(16, "impact", json!({ "symbol": "a", "depth": 2.5 })),
            call(17, "impact", json!({ "symbol": "a", "depth": "2" })),
            call(18, "impact", json!({ "symbol": "a", "min_confidence": 0 })),
            call(
                19,
                "impact",
                json!({ "symbol": "a", "min_confidence": 1.5 }),
            ),
            call(20, "impact", json!({ "symbol": "a", "include_tests": 1 })),
            call(21, "impact", json!({ "symbol": "a", "summary": "no" })),
        ]),
    );
    for reply in &replies[1..5] {
        assert_eq!(error_code(reply), -32602, "{reply}");
    }
    let messages = [
        "query is required",
        "query must be a string",
        "unknown argument; this tool takes query, repo",
        "give exactly one of symbol and id",
        "give exactly one of symbol and id",
        "source must be true or false",
        "symbol must be a string",
        "symbol is required",
        "depth must be an integer from 1 to 5",
        "depth must be an integer from 1 to 5",
        "depth must be an integer from 1 to 5",
        "depth must be an integer from 1 to 5",
        "min_confidence must be a number above 0 and at most 1",
        "min_confidence must be a number above 0 and at most 1",
        "include_tests must be true or false",
        "summary must be true or false",
    ];
    assert_eq!(replies.len(), 5 + messages.len());
    for (reply, message) in replies[5..].iter().zip(messages) {
        let envelope = envelope(reply);
        assert_eq!(
            *envelope,
            json!({
                "ok": false,
                "data": null,
                "error": { "kind": "usage", "message": message, "exit": 1 },
                "schema": 1,
            }),
            "{reply}"
        );
    }
}

#[test]
fn reads_write_nothing_and_index_repo_writes_only_the_data_directory() {
    let cwd = TempDir::new("cwd");
    let data = TempDir::new("data");
    copy_tree(
        &Path::new(FIXTURES).join("impact"),
        &cwd.path().join("impact"),
    );
    let snapshot = |dir: &Path| {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read dir") {
                let entry = entry.expect("entry");
                let meta = entry.metadata().expect("meta");
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    out.push((entry.path(), meta.len(), meta.modified().ok()));
                }
            }
        }
        out.sort();
        out
    };
    let tree = snapshot(cwd.path());
    let reads = transcript(&[
        initialize(1, "2025-11-25"),
        call(
            2,
            "find_symbols",
            json!({ "query": "root_fn", "repo": "impact" }),
        ),
        call(
            3,
            "impact",
            json!({ "symbol": "root_fn", "repo": "impact" }),
        ),
        call(4, "index_repo", json!({ "path": "impact" })),
    ]);
    let run = serve(BINS[0], &[], cwd.path(), data.path(), &reads);
    let replies = run.replies();
    assert_eq!(envelope(&replies[1])["error"]["kind"], "no_index");
    assert_eq!(error_code(&replies[3]), -32602);
    assert_eq!(snapshot(cwd.path()), tree);
    assert!(
        snapshot(data.path()).is_empty(),
        "nothing written without the flag"
    );
    let index = transcript(&[
        initialize(1, "2025-11-25"),
        call(2, "index_repo", json!({ "path": "impact" })),
        call(
            3,
            "find_symbols",
            json!({ "query": "root_fn", "repo": "impact" }),
        ),
    ]);
    let run = serve(BINS[1], &["--allow-index"], cwd.path(), data.path(), &index);
    let replies = run.replies();
    assert_eq!(envelope(&replies[1])["ok"], true, "{}", replies[1]);
    assert_eq!(
        envelope(&replies[2])["data"]["hits"][0]["path"],
        "engine/src/lib.rs"
    );
    assert_eq!(snapshot(cwd.path()), tree, "the tree is never written");
    assert!(!snapshot(data.path()).is_empty());
}

#[test]
fn no_output_line_holds_an_absolute_path() {
    let cwd = TempDir::new("cwd");
    let data = TempDir::new("data");
    copy_tree(
        &Path::new(FIXTURES).join("detail"),
        &cwd.path().join("detail"),
    );
    let input = transcript(&[
        initialize(1, "2025-11-25"),
        call(2, "index_repo", json!({ "path": "detail" })),
        call(3, "index_repo", json!({ "path": "missing" })),
        call(
            4,
            "find_symbols",
            json!({ "query": "largest", "repo": "detail" }),
        ),
        call(
            5,
            "show_symbol",
            json!({ "symbol": "largest", "repo": "detail", "source": true }),
        ),
        call(
            6,
            "show_symbol",
            json!({ "symbol": "largest", "repo": "missing" }),
        ),
        call(
            7,
            "callers",
            json!({ "symbol": "nothing_here", "repo": "detail" }),
        ),
        call(8, "callees", json!({ "symbol": "largest" })),
        call(
            9,
            "impact",
            json!({ "symbol": "largest", "repo": cwd.path().join("detail") }),
        ),
    ]);
    let run = serve(BINS[0], &["--allow-index"], cwd.path(), data.path(), &input);
    let root = cwd.path().to_str().expect("utf-8 temp path");
    let data_dir = data.path().to_str().expect("utf-8 temp path");
    assert_eq!(run.replies().len(), 9);
    for line in &run.lines {
        assert!(!line.contains(root) && !line.contains(data_dir), "{line}");
    }
    let replies = run.replies();
    assert_eq!(envelope(&replies[4])["data"]["source"]["status"], "ok");
    assert_eq!(
        envelope(&replies[8])["ok"],
        true,
        "an absolute repo is accepted"
    );
}

#[test]
fn stdout_carries_only_json_rpc_lines() {
    let cwd = TempDir::new("cwd");
    let data = TempDir::new("data");
    copy_tree(
        &Path::new(FIXTURES).join("impact"),
        &cwd.path().join("impact"),
    );
    let mut input = transcript(&[
        request(1, "tools/list", json!({})),
        initialize(2, "2025-06-18"),
        initialized(),
        call(3, "index_repo", json!({ "path": "impact" })),
        call(
            4,
            "show_symbol",
            json!({ "symbol": "root_fn", "repo": "impact", "source": true }),
        ),
        call(
            5,
            "impact",
            json!({ "symbol": "root_fn", "repo": "impact" }),
        ),
        call(
            6,
            "callers",
            json!({ "symbol": "helper", "repo": "impact" }),
        ),
        call(
            7,
            "find_symbols",
            json!({ "query": "root_fn", "repo": "nowhere" }),
        ),
        call(8, "find_symbols", json!({ "bogus": 1 })),
        call(9, "nope", json!({})),
    ]);
    input.extend_from_slice(b"{oops\n[1]\n");
    input.extend(vec![b'x'; MAX_LINE + 10]);
    input.extend_from_slice(b"\n");
    input.extend(transcript(&[request(10, "ping", json!({}))]));
    let run = serve(BINS[0], &["--allow-index"], cwd.path(), data.path(), &input);
    assert_eq!(run.code, Some(0));
    assert!(run.stderr.is_empty());
    let replies = run.replies();
    let ids: Vec<Value> = replies.iter().map(|reply| reply["id"].clone()).collect();
    assert_eq!(
        ids,
        [
            json!(1),
            json!(2),
            json!(3),
            json!(4),
            json!(5),
            json!(6),
            json!(7),
            json!(8),
            json!(9),
            json!(null),
            json!(null),
            json!(null),
            json!(10),
        ]
    );
    // Every byte on stdout belongs to one of those lines.
    let bytes: usize = run.lines.iter().map(|line| line.len() + 1).sum();
    let printed: String = run.lines.iter().map(|line| format!("{line}\n")).collect();
    assert_eq!(printed.len(), bytes);
    for line in &run.lines {
        assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
    }
}
