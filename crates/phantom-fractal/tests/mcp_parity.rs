//! Every tool's `structuredContent` against the command line's envelope
//! for the same operation and arguments, on `fixtures/impact` and
//! `fixtures/detail` (SPEC-w6 G2b, G2c).
//!
//! Each case runs the command line and the server from the same working
//! directory against the same data directory. The text item must be the
//! command line's stdout line byte for byte, and `structuredContent` must
//! parse equal to it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-mcp-parity-{label}-{}-{}",
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

/// A working directory holding copies of both fixtures, and a data
/// directory with both indexed.
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
        for name in ["impact", "detail"] {
            copy_tree(
                &Path::new(FIXTURES).join(name),
                &world.cwd.path().join(name),
            );
            let line = world.cli(&["index", name], world.cwd.path());
            assert!(line.starts_with("{\"ok\":true"), "{line}");
        }
        world
    }

    /// The command line's stdout line, newline removed.
    fn cli(&self, args: &[&str], cwd: &Path) -> String {
        let output = Command::new(BIN)
            .args(args)
            .current_dir(cwd)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let line = stdout.strip_suffix('\n').expect("one line").to_owned();
        assert!(!line.contains('\n'), "{line}");
        line
    }

    /// The server's tool results for `calls`, in order, as (text,
    /// structuredContent, isError).
    fn mcp(
        &self,
        extra: &[&str],
        cwd: &Path,
        calls: &[(&str, Value)],
    ) -> Vec<(String, Value, bool)> {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"parity\",\"version\":\"0\"}}}\n",
        );
        for (id, (name, arguments)) in calls.iter().enumerate() {
            let message = json!({
                "jsonrpc": "2.0",
                "id": id + 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            });
            input.push_str(&message.to_string());
            input.push('\n');
        }
        let mut child = Command::new(BIN)
            .arg("mcp")
            .args(extra)
            .current_dir(cwd)
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
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let replies: Vec<Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(replies.len(), calls.len() + 1);
        replies[1..]
            .iter()
            .zip(1..)
            .map(|(reply, id)| {
                assert_eq!(reply["id"], id, "{reply}");
                let result = &reply["result"];
                let content = result["content"].as_array().expect("content");
                assert_eq!(content.len(), 1);
                (
                    content[0]["text"].as_str().expect("text").to_owned(),
                    result["structuredContent"].clone(),
                    result["isError"].as_bool().expect("isError"),
                )
            })
            .collect()
    }
}

/// One tool call and the command line that must print the same envelope.
struct Case {
    tool: &'static str,
    arguments: Value,
    cli: Vec<String>,
}

fn case(tool: &'static str, arguments: Value, cli: &[&str]) -> Case {
    Case {
        tool,
        arguments,
        cli: cli.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

/// Runs every case through both, from `cwd`, and compares. Returns each
/// envelope's error kind, or `ok`, so a test also pins which cases succeed.
fn assert_parity(world: &World, cwd: &Path, extra: &[&str], cases: &[Case]) -> Vec<String> {
    let calls: Vec<(&str, Value)> = cases
        .iter()
        .map(|case| (case.tool, case.arguments.clone()))
        .collect();
    let got = world.mcp(extra, cwd, &calls);
    let mut kinds = Vec::new();
    for (case, (text, structured, is_error)) in cases.iter().zip(got) {
        let args: Vec<&str> = case.cli.iter().map(String::as_str).collect();
        let line = world.cli(&args, cwd);
        assert_eq!(text, line, "{} {}", case.tool, case.arguments);
        let envelope: Value = serde_json::from_str(&line).expect("cli envelope");
        assert_eq!(structured, envelope, "{} {}", case.tool, case.arguments);
        assert_eq!(is_error, envelope["ok"] == false, "{}", case.tool);
        kinds.push(
            envelope["error"]["kind"]
                .as_str()
                .unwrap_or("ok")
                .to_owned(),
        );
    }
    kinds
}

/// The stable key `show` gives `symbol` in `repo`.
fn stable_key(world: &World, symbol: &str, repo: &str) -> String {
    let line = world.cli(&["show", symbol, "--repo", repo], world.cwd.path());
    let envelope: Value = serde_json::from_str(&line).expect("show");
    envelope["data"]["symbol"]["stable_key"]
        .as_str()
        .expect("stable key")
        .to_owned()
}

#[test]
fn every_read_tool_matches_the_cli_on_fixtures_impact() {
    let world = World::new();
    let key = stable_key(&world, "root_fn", "impact");
    let cases = [
        case(
            "find_symbols",
            json!({ "query": "root_fn", "repo": "impact" }),
            &["find", "root_fn", "--repo", "impact"],
        ),
        case(
            "find_symbols",
            json!({ "query": "helper", "repo": "impact" }),
            &["find", "helper", "--repo", "impact"],
        ),
        case(
            "find_symbols",
            json!({ "query": "absent", "repo": "impact" }),
            &["find", "absent", "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "root_fn", "repo": "impact" }),
            &["show", "root_fn", "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "id": key, "repo": "impact" }),
            &["show", "--id", &key, "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "area", "repo": "impact", "source": true }),
            &["show", "area", "--repo", "impact", "--source"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "area", "repo": "impact", "source": false }),
            &["show", "area", "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "helper", "repo": "impact" }),
            &["show", "helper", "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "absent", "repo": "impact" }),
            &["show", "absent", "--repo", "impact"],
        ),
        case(
            "show_symbol",
            json!({ "id": "no#such#key", "repo": "impact" }),
            &["show", "--id", "no#such#key", "--repo", "impact"],
        ),
        case(
            "callers",
            json!({ "symbol": "root_fn", "repo": "impact" }),
            &["callers", "root_fn", "--repo", "impact"],
        ),
        case(
            "callers",
            json!({ "symbol": "helper", "repo": "impact" }),
            &["callers", "helper", "--repo", "impact"],
        ),
        case(
            "callees",
            json!({ "symbol": "run", "repo": "impact" }),
            &["callees", "run", "--repo", "impact"],
        ),
        case(
            "callees",
            json!({ "symbol": "absent", "repo": "impact" }),
            &["callees", "absent", "--repo", "impact"],
        ),
        case(
            "impact",
            json!({ "symbol": "root_fn", "repo": "impact" }),
            &["impact", "root_fn", "--repo", "impact"],
        ),
        case(
            "impact",
            json!({ "symbol": "area", "repo": "impact", "depth": 2, "min_confidence": 0.9, "include_tests": false, "summary": true }),
            &[
                "impact",
                "area",
                "--repo",
                "impact",
                "--depth",
                "2",
                "--min-confidence",
                "0.9",
                "--no-tests",
                "--summary",
            ],
        ),
        case(
            "impact",
            json!({ "symbol": "hidden", "repo": "impact", "depth": 5, "min_confidence": 1, "include_tests": true, "summary": false }),
            &[
                "impact",
                "hidden",
                "--repo",
                "impact",
                "--depth",
                "5",
                "--min-confidence",
                "1",
            ],
        ),
        case(
            "impact",
            json!({ "symbol": "absent", "repo": "impact" }),
            &["impact", "absent", "--repo", "impact"],
        ),
        case(
            "find_symbols",
            json!({ "query": "root_fn", "repo": "missing" }),
            &["find", "root_fn", "--repo", "missing"],
        ),
        case(
            "find_symbols",
            json!({ "query": "root_fn" }),
            &["find", "root_fn"],
        ),
    ];
    let kinds = assert_parity(&world, world.cwd.path(), &[], &cases);
    let kinds = kinds.iter().map(String::as_str).collect::<Vec<_>>();
    let want = [
        "ok",
        "ok",
        "ok",
        "ok",
        "ok",
        "ok",
        "ok",
        "ambiguous",
        "not_found",
        "not_found",
        "ok",
        "ambiguous",
        "ok",
        "not_found",
        "ok",
        "ok",
        "ok",
        "not_found",
        "not_found",
        "no_index",
    ];
    assert_eq!(kinds, want);
}

#[test]
fn every_read_tool_matches_the_cli_on_fixtures_detail() {
    let world = World::new();
    let mut cases = Vec::new();
    for name in [
        "largest",
        "connect",
        "Meters",
        "Heading",
        "Grid",
        "Area",
        "configure_everything",
        "summarize",
    ] {
        cases.push(case(
            "find_symbols",
            json!({ "query": name, "repo": "detail" }),
            &["find", name, "--repo", "detail"],
        ));
        cases.push(case(
            "show_symbol",
            json!({ "symbol": name, "repo": "detail" }),
            &["show", name, "--repo", "detail"],
        ));
        cases.push(case(
            "show_symbol",
            json!({ "symbol": name, "repo": "detail", "source": true }),
            &["show", name, "--repo", "detail", "--source"],
        ));
    }
    cases.push(case(
        "callers",
        json!({ "symbol": "area", "repo": "detail" }),
        &["callers", "area", "--repo", "detail"],
    ));
    cases.push(case(
        "callees",
        json!({ "symbol": "largest", "repo": "detail" }),
        &["callees", "largest", "--repo", "detail"],
    ));
    cases.push(case(
        "impact",
        json!({ "symbol": "area", "repo": "detail" }),
        &["impact", "area", "--repo", "detail"],
    ));
    let kinds = assert_parity(&world, world.cwd.path(), &[], &cases);
    assert!(kinds.iter().all(|kind| kind == "ok"), "{kinds:?}");
}

#[test]
fn repo_defaults_to_the_server_directory_and_a_relative_repo_resolves_against_it() {
    let world = World::new();
    // From a directory inside the indexed tree, `..` names the tree, and no
    // `repo` names the server's own directory, which has no index.
    let inside = world.cwd.path().join("impact/engine");
    let cases = [
        case(
            "find_symbols",
            json!({ "query": "root_fn", "repo": ".." }),
            &["find", "root_fn", "--repo", ".."],
        ),
        case(
            "callers",
            json!({ "symbol": "root_fn", "repo": "../." }),
            &["callers", "root_fn", "--repo", "../."],
        ),
        case(
            "find_symbols",
            json!({ "query": "root_fn" }),
            &["find", "root_fn"],
        ),
    ];
    let kinds = assert_parity(&world, &inside, &[], &cases);
    assert_eq!(kinds, ["ok", "ok", "no_index"]);
    let root = world.cwd.path().join("impact");
    let cases = [
        case(
            "find_symbols",
            json!({ "query": "root_fn" }),
            &["find", "root_fn"],
        ),
        case(
            "impact",
            json!({ "symbol": "root_fn" }),
            &["impact", "root_fn"],
        ),
        case(
            "show_symbol",
            json!({ "symbol": "root_fn", "repo": "." }),
            &["show", "root_fn", "--repo", "."],
        ),
    ];
    let kinds = assert_parity(&world, &root, &[], &cases);
    assert_eq!(kinds, ["ok", "ok", "ok"]);
    let found = world.mcp(
        &[],
        &root,
        &[("find_symbols", json!({ "query": "root_fn" }))],
    );
    assert_eq!(
        found[0].1["ok"], true,
        "the server's directory is the indexed tree"
    );
}

#[test]
fn index_repo_matches_the_cli_index() {
    // A fresh tree in a fresh data directory, once for each side.
    let fresh = TempDir::new("fresh");
    copy_tree(
        &Path::new(FIXTURES).join("detail"),
        &fresh.path().join("detail"),
    );
    let cli_data = TempDir::new("cli-data");
    let output = Command::new(BIN)
        .args(["index", "detail"])
        .current_dir(fresh.path())
        .env("PHANTOM_FRACTAL_DATA", cli_data.path())
        .output()
        .expect("cli index");
    let cli_first = String::from_utf8(output.stdout).expect("utf-8");
    let mcp_data = World {
        cwd: TempDir::new("unused"),
        data: TempDir::new("mcp-data"),
    };
    let got = mcp_data.mcp(
        &["--allow-index"],
        fresh.path(),
        &[
            ("index_repo", json!({ "path": "detail" })),
            ("index_repo", json!({ "path": "detail" })),
            ("index_repo", json!({ "path": "absent" })),
        ],
    );
    assert_eq!(got[0].0, cli_first.trim_end());
    // The second pass writes nothing, as the command line's does.
    let cli_second = mcp_data.cli(&["index", "detail"], fresh.path());
    assert_eq!(got[1].0, cli_second);
    assert_eq!(got[2].0, mcp_data.cli(&["index", "absent"], fresh.path()));
    assert!(got[2].2);
}
