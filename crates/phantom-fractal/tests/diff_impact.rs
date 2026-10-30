//! `fract impact --diff` and the MCP `diff_impact` tool (SPEC-w9 G2b), on
//! the co-change `history` as a git root, built by `support`.
//!
//! The changed symbols, the files with none, the base and the reasons are
//! checked against values computed here; the walk is `impact`'s, so its
//! hits are checked against `fract impact` on the same symbol. A diff must
//! write nothing: not the tree, not `.git`, not the database or its WAL.

mod support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use support::{
    Repo, Script, TempDir, copy_fixture, database, history, raw_keys, snapshot, value_at,
};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const STAGED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"staged and unstaged diffs need the git index, which this build does not read\",\"exit\":1},\"schema\":1}";

const NOT_GIT: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"impact --diff needs a git root: the indexed tree is not the top of a git work tree\",\"exit\":1},\"schema\":1}";

const UNKNOWN: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\",\"message\":\"the base is not a branch, a tag, or a commit id of at least 7 hex digits\",\"exit\":1},\"schema\":1}";

const REFUSED: &str = "{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"no_index\",\"message\":\"repo is not an indexed root of this server\",\"exit\":2},\"schema\":1}";

/// The history repo and a data directory, indexed.
struct World {
    repo: Repo,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            repo: Repo::new("diff", &history()),
            data: TempDir::new("diff-data"),
        };
        world.ok(&["index", "."]);
        world
    }

    fn root(&self) -> &Path {
        &self.repo.root
    }

    fn line_with(&self, bin: &str, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(bin)
            .args(args)
            .current_dir(self.root())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run cli");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let line = stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned();
        assert!(!line.contains('\n'), "{line}");
        (output.status.code(), line)
    }

    fn line(&self, args: &[&str]) -> (Option<i32>, String) {
        self.line_with(BINS[0], args)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, line) = self.line(args);
        assert_eq!(code, Some(0), "{args:?}: {line}");
        serde_json::from_str(&line).expect("envelope")
    }

    fn write(&self, path: &str, body: &str) {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        fs::write(path, body).expect("write");
    }

    /// Each tool call's text item, from one server rooted at the repo,
    /// checked against its `structuredContent`; the `tools/list` result
    /// first.
    fn mcp(&self, extra: &[&str], calls: &[Value]) -> (Vec<Value>, Vec<String>) {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"diff\",\"version\":\"0\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
        );
        for (id, arguments) in (2..).zip(calls) {
            let message = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": "diff_impact", "arguments": arguments },
            });
            input.push_str(&format!("{message}\n"));
        }
        let mut child = Command::new(BINS[0])
            .arg("mcp")
            .args(extra)
            .current_dir(self.root())
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
                assert_eq!(result["isError"], json!(parsed["ok"] == false));
                text.to_owned()
            })
            .collect();
        (tools, texts)
    }

    /// The tree with `.git`, and the database with its WAL, as bytes and
    /// mtimes.
    fn state(&self) -> String {
        format!(
            "{:?}{:?}",
            snapshot(self.root()),
            database(self.data.path())
        )
    }
}

fn changed(path: &str, name: &str, kind: &str, change: &str, in_index: bool) -> Value {
    json!({
        "path": path,
        "key": format!("{path}#{name}#{kind}#0"),
        "name": name,
        "change": change,
        "in_index": in_index,
    })
}

fn reasons(data: &Value) -> Vec<String> {
    data["impact"]["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|reason| reason["reason"].as_str().expect("reason").to_owned())
        .collect()
}

#[test]
fn a_clean_tree_changes_nothing_and_the_keys_are_in_spec_order() {
    let world = World::new();
    let head = world.repo.rev("HEAD");
    for bin in BINS {
        let (code, line) = world.line_with(bin, &["impact", "--diff"]);
        assert_eq!(code, Some(0), "{line}");
        assert_eq!(
            raw_keys(&line, 0),
            ["ok", "data", "meta", "error", "schema"]
        );
        let data_at = value_at(&line, "data", 0);
        assert_eq!(
            raw_keys(&line, data_at),
            [
                "base",
                "changed",
                "files_only",
                "manifest_changed",
                "impact"
            ]
        );
        assert_eq!(
            raw_keys(&line, value_at(&line, "base", data_at)),
            ["kind", "ref", "oid"]
        );
        assert_eq!(
            raw_keys(&line, value_at(&line, "impact", data_at)),
            [
                "risk", "exact", "reasons", "counts", "groups", "tests", "options"
            ]
        );
        let data = &serde_json::from_str::<Value>(&line).expect("envelope")["data"];
        assert_eq!(
            data["base"],
            json!({ "kind": "head", "ref": null, "oid": head })
        );
        assert_eq!(data["changed"], json!([]));
        assert_eq!(data["files_only"], json!([]));
        assert_eq!(data["manifest_changed"], false);
        assert_eq!(data["impact"]["groups"], json!([]));
        // `--all` is the default.
        assert_eq!(
            world.line_with(bin, &["impact", "--diff", "--all"]),
            (code, line)
        );
    }
}

#[test]
fn changed_symbols_files_and_their_impact() {
    let world = World::new();
    world.write(
        "src/b.rs",
        "/// b's function.\npub fn fb() { let _ = 1; }\n",
    );
    world.write("src/z.rs", "pub fn fz() {}\n");
    fs::remove_file(world.root().join("src/g.rs")).expect("remove g");
    world.write("README.md", "notes\n");
    world.write(
        "src/c.rs",
        "/// c's function.\npub fn fc() {}\n// revision 99\n",
    );
    let manifest = fs::read_to_string(world.root().join("Cargo.toml")).expect("manifest");
    world.write("Cargo.toml", &format!("{manifest}\n[features]\nx = []\n"));
    let before = world.state();
    let (code, line) = world.line(&["impact", "--diff"]);
    assert_eq!(code, Some(0), "{line}");
    let data = &serde_json::from_str::<Value>(&line).expect("envelope")["data"];
    assert_eq!(
        data["changed"],
        json!([
            changed("src/b.rs", "fb", "function", "modified", true),
            changed("src/g.rs", "fg", "function", "removed", true),
            changed("src/z.rs", "fz", "function", "added", false),
        ])
    );
    // A `.rs` change that touches no symbol's bytes, a non-`.rs` file and
    // the manifest, by path.
    assert_eq!(
        data["files_only"],
        json!([
            { "path": "Cargo.toml", "change": "modified" },
            { "path": "README.md", "change": "added" },
            { "path": "src/c.rs", "change": "modified" },
        ])
    );
    assert_eq!(data["manifest_changed"], true);
    // The tree changed since the index, and `fz` is not in it.
    assert_eq!(
        reasons(data),
        [
            "external_users_possible",
            "stale_index",
            "unindexed_changes"
        ]
    );
    let unindexed = &data["impact"]["reasons"][2];
    assert_eq!(unindexed["count"], 1);
    // `top` calls `fb`: the walk is `impact`'s from `fb` and `fg`.
    let fb = world.ok(&["impact", "fb"])["data"]["groups"].clone();
    assert_eq!(data["impact"]["groups"], fb);
    assert_eq!(world.state(), before, "a diff wrote");
    // The same diff twice prints the same.
    assert_eq!(world.line(&["impact", "--diff"]), (code, line));
}

#[test]
fn the_reasons_appear_only_when_they_apply() {
    let world = World::new();
    world.write(
        "src/b.rs",
        "/// b's function.\npub fn fb() { let _ = 1; }\n",
    );
    let data = world.ok(&["impact", "--diff"])["data"].clone();
    assert_eq!(reasons(&data), ["external_users_possible", "stale_index"]);
    world.ok(&["index", "."]);
    let data = world.ok(&["impact", "--diff"])["data"].clone();
    assert_eq!(reasons(&data), ["external_users_possible"]);
    assert_eq!(data["changed"][0]["in_index"], true);

    world.write("src/z.rs", "pub fn fz() {}\n");
    let data = world.ok(&["impact", "--diff"])["data"].clone();
    assert_eq!(
        reasons(&data),
        [
            "external_users_possible",
            "stale_index",
            "unindexed_changes"
        ]
    );
    world.ok(&["index", "."]);
    let data = world.ok(&["impact", "--diff"])["data"].clone();
    assert_eq!(reasons(&data), ["external_users_possible"]);
    assert_eq!(data["changed"][1]["in_index"], true);

    // Removed from the tree and re-indexed: no longer in the index.
    fs::remove_file(world.root().join("src/g.rs")).expect("remove g");
    world.ok(&["index", "."]);
    let data = world.ok(&["impact", "--diff"])["data"].clone();
    assert_eq!(
        reasons(&data),
        ["external_users_possible", "unindexed_changes"]
    );
}

#[test]
fn against_names_a_branch_a_tag_or_a_commit_id() {
    let world = World::new();
    let old = world.repo.rev("HEAD~3");
    world.repo.git(&["tag", "v-old", &old], None);
    world.repo.git(&["branch", "older", &old], None);
    let base =
        |against: &str| world.ok(&["impact", "--diff", "--against", against])["data"].clone();
    let tagged = base("v-old");
    assert_eq!(
        tagged["base"],
        json!({ "kind": "ref", "ref": "v-old", "oid": old })
    );
    // The history changes only revision comments, so the files since the
    // base changed and no symbol did.
    assert_eq!(tagged["changed"], json!([]));
    assert_eq!(
        tagged["files_only"],
        json!([{ "path": "src/f.rs", "change": "modified" }])
    );
    for against in ["older", &old[..7], &old[..12], &old] {
        let data = base(against);
        assert_eq!(data["base"]["oid"], json!(old), "{against}");
        assert_eq!(data["base"]["ref"], json!(against), "{against}");
        assert_eq!(data["files_only"], tagged["files_only"], "{against}");
    }
    let head = base("main");
    assert_eq!(head["files_only"], json!([]));
    for against in ["nope", "main~1", &old[..6], "zzzzzzz", ""] {
        assert_eq!(
            world.line(&["impact", "--diff", "--against", against]),
            (Some(1), UNKNOWN.to_owned()),
            "{against}"
        );
    }
}

#[test]
fn staged_unstaged_no_git_and_no_index_are_refused() {
    let world = World::new();
    for bin in BINS {
        for args in [
            &["impact", "--diff", "--staged"][..],
            &["impact", "--diff", "--unstaged"],
            &["impact", "--diff", "--staged", "--against", "main"],
        ] {
            assert_eq!(
                world.line_with(bin, args),
                (Some(1), STAGED.to_owned()),
                "{args:?}"
            );
        }
    }
    // clap refuses these, on stderr.
    for args in [
        &["impact", "--diff", "fa"][..],
        &["impact", "--against", "main", "fa"],
        &["impact", "--all", "fa"],
        &["impact", "--diff", "--all", "--against", "main"],
        &["impact", "--staged", "fa"],
        &["impact", "--against", "main"],
        &["impact", "--all"],
    ] {
        let output = Command::new(BINS[0])
            .args(args)
            .current_dir(world.root())
            .env("PHANTOM_FRACTAL_DATA", world.data.path())
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
    // A root without git, indexed; then a git root with no index.
    let dir = TempDir::new("diff-fixture");
    let tree: PathBuf = dir.path().join("graph");
    copy_fixture("graph", &tree);
    let path = tree.to_str().expect("utf-8");
    assert_eq!(world.line(&["index", path]).0, Some(0));
    assert_eq!(
        world.line(&["impact", "--diff", "--repo", path]),
        (Some(1), NOT_GIT.to_owned())
    );
    let fresh = World {
        repo: Repo::new("diff-unindexed", &history()),
        data: TempDir::new("diff-data"),
    };
    let (code, line) = fresh.line(&["impact", "--diff"]);
    assert_eq!(code, Some(2), "{line}");
}

#[test]
fn diff_impact_is_the_cli_line_and_keeps_to_the_roots() {
    let world = World::new();
    world.write(
        "src/b.rs",
        "/// b's function.\npub fn fb() { let _ = 1; }\n",
    );
    world.write("README.md", "notes\n");
    let old = world.repo.rev("HEAD~2");
    let pairs: Vec<(Value, Vec<&str>)> = vec![
        (json!({}), vec!["impact", "--diff"]),
        (
            json!({ "against": "main" }),
            vec!["impact", "--diff", "--against", "main"],
        ),
        (
            json!({ "against": &old[..9], "depth": 5, "min_confidence": 0.5 }),
            vec![
                "impact",
                "--diff",
                "--against",
                &old[..9],
                "--depth",
                "5",
                "--min-confidence",
                "0.5",
            ],
        ),
        (
            json!({ "include_tests": false }),
            vec!["impact", "--diff", "--no-tests"],
        ),
        (
            json!({ "summary_only": true }),
            vec!["impact", "--diff", "--summary"],
        ),
        (
            json!({ "max_tokens": 300 }),
            vec!["impact", "--diff", "--max-tokens", "300"],
        ),
        (
            json!({ "against": "nope" }),
            vec!["impact", "--diff", "--against", "nope"],
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
        .position(|name| *name == "diff_impact")
        .expect("listed");
    assert_eq!(
        names[at - 1..=at + 1],
        ["cochange", "diff_impact", "symbol_path"]
    );
    let tool = &tools[at];
    assert_eq!(
        tool["annotations"],
        json!({ "readOnlyHint": true, "idempotentHint": true })
    );
    let schema = &tool["inputSchema"];
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!([]));
    let mut keys: Vec<&String> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "against",
            "depth",
            "include_tests",
            "max_tokens",
            "min_confidence",
            "repo",
            "summary_only"
        ]
    );

    // Not a root, bad arguments, and no way to ask for a staged diff.
    let other = TempDir::new("diff-other");
    let (_, texts) = world.mcp(
        &["--root", other.path().to_str().expect("utf-8")],
        &[
            json!({}),
            json!({ "repo": world.root().to_str().expect("utf-8") }),
        ],
    );
    assert_eq!(texts, [REFUSED, REFUSED]);
    let (_, texts) = world.mcp(
        &[],
        &[
            json!({ "staged": true }),
            json!({ "against": 7 }),
            json!({ "depth": 9 }),
            json!({ "summary_only": "yes" }),
        ],
    );
    for text in &texts {
        assert!(
            text.starts_with("{\"ok\":false,\"data\":null,\"error\":{\"kind\":\"usage\""),
            "{text}"
        );
    }
}

#[test]
fn no_reply_holds_an_absolute_path() {
    let world = World::new();
    world.write(
        "src/b.rs",
        "/// b's function.\npub fn fb() { let _ = 1; }\n",
    );
    let root = world.root().to_str().expect("utf-8").to_owned();
    let data = world.data.path().to_str().expect("utf-8").to_owned();
    let mut script = Script::after_main();
    script.commit(
        support::HEAD + support::DAY,
        &[("src/new.rs", "pub fn fresh() {}\n".to_owned())],
    );
    world.repo.import(&script);
    for args in [
        &["impact", "--diff"][..],
        &["impact", "--diff", "--against", "nope"],
        &["impact", "--diff", "--against", "main"],
        &["impact", "--diff", "--staged"],
    ] {
        let line = world.line(args).1;
        assert!(!line.contains(&root) && !line.contains(&data), "{line}");
        assert!(!line.contains("\"path\":\"/"), "{line}");
    }
}

#[test]
fn summary_and_no_tests_leave_out_their_keys() {
    let world = World::new();
    world.write(
        "src/b.rs",
        "/// b's function.\npub fn fb() { let _ = 1; }\n",
    );
    for (args, want) in [
        (
            &["impact", "--diff"][..],
            &[
                "risk", "exact", "reasons", "counts", "groups", "tests", "options",
            ][..],
        ),
        (
            &["impact", "--diff", "--summary"],
            &["risk", "exact", "reasons", "counts", "options"],
        ),
        (
            &["impact", "--diff", "--no-tests"],
            &["risk", "exact", "reasons", "counts", "groups", "options"],
        ),
    ] {
        let (code, line) = world.line(args);
        assert_eq!(code, Some(0), "{line}");
        let data_at = value_at(&line, "data", 0);
        assert_eq!(
            raw_keys(&line, value_at(&line, "impact", data_at)),
            want,
            "{args:?}"
        );
    }
    let summary = world.ok(&["impact", "--diff", "--summary"])["data"]["impact"].clone();
    let full = world.ok(&["impact", "--diff"])["data"]["impact"].clone();
    assert_eq!(summary["counts"], full["counts"]);
    assert_eq!(summary["reasons"], full["reasons"]);
}
