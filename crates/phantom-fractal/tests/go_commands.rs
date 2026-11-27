//! Go name forms through every command line verb and the MCP tools
//! (SPEC-w17 G3c), on copies of `fixtures/go-basic`,
//! `fixtures/go-embed-iface` and `fixtures/go-testpkg`.
//!
//! A Go name is spelled with `.`: `pkg.Func`, `Type.Method`,
//! `import/path.Type.Method`, `dir/f.go::Type.Method` and `dir/f.go:LINE`.
//! Whatever a reply prints must read back as the same symbol.

mod support;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use support::{HEAD, Repo, Script, TempDir, copy_fixture};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

/// A tree and its own data directory.
struct Tree {
    tmp: TempDir,
    repo: PathBuf,
    /// Held so the git root outlives the test.
    _git: Option<Repo>,
}

impl Tree {
    /// `fixtures/<name>`, copied and indexed.
    fn fixture(name: &str) -> Self {
        let tree = Self::empty(name);
        copy_fixture(name, &tree.repo);
        tree.ok(&["index", "."]);
        tree
    }

    fn empty(label: &str) -> Self {
        let tmp = TempDir::new(label);
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo dir");
        Self {
            tmp,
            repo,
            _git: None,
        }
    }

    /// `fixtures/go-basic` as one commit of a git root, indexed.
    fn committed() -> Self {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/go-basic");
        let files: Vec<(&str, String)> = [
            "go.mod",
            "cmd/app/main.go",
            "internal/core/core.go",
            "lib/lib.go",
        ]
        .into_iter()
        .map(|path| {
            let body = fs::read_to_string(fixture.join(path)).expect("fixture file");
            (path, body)
        })
        .collect();
        let mut script = Script::default();
        script.commit(HEAD, &files);
        let git = Repo::new("go-commands", &script);
        let mut tree = Self::empty("go-committed");
        tree.repo = git.root.clone();
        tree._git = Some(git);
        tree.ok(&["index", "."]);
        tree
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .current_dir(&self.repo)
            .env("PHANTOM_FRACTAL_DATA", self.tmp.path().join("data"));
        command
    }

    /// The stdout line, newline removed.
    fn line(&self, args: &[&str]) -> String {
        let out = self.command().args(args).output().expect("run");
        let stdout = String::from_utf8(out.stdout).expect("utf-8");
        stdout.trim_end_matches('\n').to_owned()
    }

    fn envelope(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.line(args)).expect("JSON envelope")
    }

    fn ok(&self, args: &[&str]) -> Value {
        let envelope = self.envelope(args);
        assert_eq!(envelope["ok"], true, "{args:?}: {envelope}");
        envelope["data"].clone()
    }

    /// The path and start line of the one symbol `name` names.
    fn at(&self, name: &str) -> (String, u64) {
        let data = self.ok(&["show", "--", name]);
        (
            text(&data["symbol"]["path"]),
            data["symbol"]["line"].as_u64().expect("line"),
        )
    }

    /// The results of `calls` from one MCP server rooted at the tree.
    fn mcp(&self, calls: &[(&str, Value)]) -> Vec<String> {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"go\",\"version\":\"0\"}}}\n",
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
        let mut child = self
            .command()
            .args(["mcp", "--root"])
            .arg(&self.repo)
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
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let replies: Vec<Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(replies.len(), calls.len() + 1);
        replies[1..]
            .iter()
            .map(|reply| text(&reply["result"]["content"][0]["text"]))
            .collect()
    }
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not text: {value}"))
        .to_owned()
}

/// `key` of every row of `rows`.
fn column(rows: &Value, key: &str) -> Vec<String> {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|row| text(&row[key]))
        .collect()
}

const BOOT: &str = "example.com/gobasic/lib.Svc.Boot";
const LEAF: &str = "example.com/gobasic/internal/core.Leaf";

#[test]
fn every_go_name_form_names_its_symbol() {
    let tree = Tree::fixture("go-basic");
    let cases: [(&str, u64, &[&str]); 5] = [
        (
            "internal/core/core.go",
            3,
            &[
                "Leaf",
                "core.Leaf",
                LEAF,
                "internal/core/core.go::Leaf",
                "internal/core/core.go:3",
                "./internal/core/core.go:3",
            ],
        ),
        (
            "lib/lib.go",
            11,
            &[
                "Boot",
                "Svc.Boot",
                "lib.Svc.Boot",
                BOOT,
                "lib/lib.go::Svc.Boot",
                "./lib/lib.go::Svc.Boot",
                "lib/lib.go:11",
            ],
        ),
        (
            "lib/lib.go",
            21,
            &["lib.Compute", "example.com/gobasic/lib.Compute"],
        ),
        (
            "cmd/app/main.go",
            5,
            &["app.Run", "example.com/gobasic/cmd/app.Run"],
        ),
        (
            "cmd/app/main.go",
            9,
            &["main.main", "cmd/app/main.go::main"],
        ),
    ];
    for (path, line, forms) in cases {
        for form in forms {
            assert_eq!(tree.at(form), (path.to_owned(), line), "{form}");
        }
    }
}

#[test]
fn every_verb_takes_a_go_name_and_prints_it_with_dots() {
    let tree = Tree::fixture("go-basic");

    let callers = tree.ok(&["callers", "core.Leaf"]);
    assert_eq!(callers["symbol"]["name"], LEAF);
    assert_eq!(
        column(&callers["hits"], "name"),
        ["example.com/gobasic/internal/core.Mid"]
    );
    assert_eq!(
        column(&callers["hits"], "display"),
        ["internal/core/core.go::Mid"]
    );

    let callees = tree.ok(&["callees", "example.com/gobasic/lib.Compute"]);
    assert!(column(&callees["hits"], "name").contains(&BOOT.to_owned()));
    assert!(column(&callees["hits"], "display").contains(&"lib/lib.go::Svc.Boot".to_owned()));

    let impact = tree.ok(&["impact", "internal/core/core.go:3"]);
    assert_eq!(impact["symbol"]["name"], LEAF);
    assert_eq!(impact["exact"], true, "{impact}");
    assert_eq!(
        column(&impact["groups"][0]["hits"], "name"),
        ["example.com/gobasic/internal/core.Mid"]
    );

    for (from, to) in [
        ("main.main", "core.Leaf"),
        ("cmd/app/main.go::main", LEAF),
        ("cmd/app/main.go:9", "internal/core/core.go:3"),
    ] {
        let path = tree.ok(&["path", from, to]);
        assert_eq!(path["found"], true, "{from} {to}: {path}");
        assert_eq!(path["hops"].as_array().expect("hops").len(), 4);
    }

    let found = tree.ok(&["find", "Boot"]);
    assert_eq!(column(&found["hits"], "name"), [BOOT]);

    let shown = tree.ok(&["show", "Svc.Boot"]);
    assert_eq!(shown["symbol"]["qual_name"], BOOT);
    assert_eq!(
        column(&shown["callers"]["hits"], "name"),
        ["example.com/gobasic/lib.Compute"]
    );

    let ranked = tree.ok(&["rank", "--focus", "lib/lib.go", "--limit", "100"]);
    assert!(column(&ranked["hits"], "qual_name").contains(&BOOT.to_owned()));

    let map = tree.ok(&["map", "--budget", "600", "--focus", "lib/lib.go"]);
    assert!(text(&map["text"]).contains("lib/lib.go"), "{map}");

    let status = tree.ok(&["status"]);
    assert_eq!(status["stale"], false, "{status}");
}

#[test]
fn every_name_a_reply_prints_reads_back_as_its_symbol() {
    for fixture in ["go-basic", "go-embed-iface", "go-testpkg"] {
        let tree = Tree::fixture(fixture);
        let ranked = tree.ok(&["rank", "--limit", "100"]);
        let hits = ranked["hits"].as_array().expect("hits");
        assert!(hits.len() > 5, "{fixture}: {ranked}");
        for hit in hits {
            let qual = text(&hit["qual_name"]);
            assert!(!qual.contains("::"), "{qual}");
            let path = text(&hit["path"]);
            let want = (path.clone(), hit["line"].as_u64().expect("line"));
            assert_eq!(tree.at(&qual), want, "{fixture}: {qual}");
            // The file form: the owner and name, cut from the right.
            let keep = if hit["kind"] == "method" { 2 } else { 1 };
            let mut tail: Vec<&str> = qual.rsplit('.').take(keep).collect();
            tail.reverse();
            let by_file = format!("{path}::{}", tail.join("."));
            assert_eq!(tree.at(&by_file), want, "{fixture}: {by_file}");
        }
    }
}

#[test]
fn ambiguous_and_not_found_spell_go_names_and_retry_with_them() {
    let embed = Tree::fixture("go-embed-iface");
    let envelope = embed.envelope(&["show", "Area"]);
    assert_eq!(envelope["error"]["kind"], "ambiguous", "{envelope}");
    let candidates = &envelope["data"]["candidates"];
    assert!(
        column(candidates, "name").contains(&"example.com/goembed/shapes.Rect.Area".to_owned()),
        "{envelope}"
    );
    let displays = column(candidates, "display");
    assert!(displays.contains(&"shapes/shapes.go::Rect.Area".to_owned()));
    for display in &displays {
        assert!(display.starts_with("shapes/shapes.go::"), "{display}");
        assert_eq!(embed.at(display).0, "shapes/shapes.go", "{display}");
    }

    let basic = Tree::fixture("go-basic");
    let envelope = basic.envelope(&["callers", "nope.Boot"]);
    assert_eq!(envelope["error"]["kind"], "not_found", "{envelope}");
    assert_eq!(column(&envelope["data"]["did_you_mean"], "name"), [BOOT]);
    assert_eq!(
        envelope["data"]["retry"],
        "phantom-fractal callers -- lib/lib.go::Svc.Boot"
    );
    let retried = basic.ok(&["callers", "--", "lib/lib.go::Svc.Boot"]);
    assert_eq!(retried["symbol"]["name"], BOOT);
}

/// Build-excluded files do not make a fresh index stale, so `path:line`
/// is answered, and an external test package's name reads back.
#[test]
fn a_tree_with_build_excluded_files_is_fresh_after_index() {
    let tree = Tree::fixture("go-testpkg");
    let status = tree.ok(&["status"]);
    assert_eq!(status["stale"], false, "{status}");
    let envelope = tree.envelope(&["show", "internal/calc/calc.go:3"]);
    assert_eq!(envelope["ok"], true, "{envelope}");
    assert_eq!(envelope["meta"]["stale"], false, "{envelope}");
    assert_eq!(
        envelope["data"]["symbol"]["qual_name"],
        "example.com/gotestpkg/internal/calc.Add"
    );
    assert_eq!(
        tree.at("example.com/gotestpkg/internal/calc_test.TestExternal"),
        ("internal/calc/calc_external_test.go".to_owned(), 9)
    );
}

#[test]
fn diff_impact_and_cochange_answer_for_a_go_tree() {
    let tree = Tree::committed();
    let file = tree.repo.join("lib/lib.go");
    let body = fs::read_to_string(&file).expect("lib.go");
    fs::write(&file, body.replace("return 7", "return 8")).expect("edit");

    let diff = tree.ok(&["impact", "--diff"]);
    assert_eq!(diff["changed"][0]["path"], "lib/lib.go", "{diff}");
    assert_eq!(diff["changed"][0]["key"], "lib/lib.go#Svc::Boot#method#0");
    assert_eq!(
        column(&diff["impact"]["groups"][0]["hits"], "name"),
        ["example.com/gobasic/lib.Compute"]
    );

    let counted = tree.ok(&["cochange", "--count", "lib/lib.go"]);
    assert!(
        column(&counted["partners"], "path").contains(&"internal/core/core.go".to_owned()),
        "{counted}"
    );
    let decayed = tree.ok(&["cochange", "lib/lib.go"]);
    assert_eq!(decayed["file"], "lib/lib.go", "{decayed}");
}

#[test]
fn mcp_tools_take_the_same_go_names_and_reply_as_the_cli() {
    let tree = Tree::committed();
    let cases: [(&str, Value, &[&str]); 9] = [
        ("show_symbol", json!({ "symbol": BOOT }), &["show", BOOT]),
        (
            "callers",
            json!({ "symbol": "lib/lib.go::Svc.Boot" }),
            &["callers", "lib/lib.go::Svc.Boot"],
        ),
        (
            "callees",
            json!({ "symbol": "lib.Compute" }),
            &["callees", "lib.Compute"],
        ),
        (
            "impact",
            json!({ "symbol": "internal/core/core.go:3" }),
            &["impact", "internal/core/core.go:3"],
        ),
        (
            "symbol_path",
            json!({ "from": "main.main", "to": LEAF }),
            &["path", "main.main", LEAF],
        ),
        (
            "find_symbols",
            json!({ "query": "Boot" }),
            &["find", "Boot"],
        ),
        ("top_ranked", json!({ "kind": "symbols" }), &["rank"]),
        (
            "repo_map",
            json!({ "budget": 600 }),
            &["map", "--budget", "600"],
        ),
        ("diff_impact", json!({}), &["impact", "--diff"]),
    ];
    let calls: Vec<(&str, Value)> = cases
        .iter()
        .map(|(tool, arguments, _)| (*tool, arguments.clone()))
        .collect();
    let replies = tree.mcp(&calls);
    for ((tool, _, cli), reply) in cases.iter().zip(&replies) {
        assert_eq!(reply, &tree.line(cli), "{tool}");
        let envelope: Value = serde_json::from_str(reply).expect("envelope");
        assert_eq!(envelope["ok"], true, "{tool}: {envelope}");
        assert!(!reply.contains("gobasic/lib::"), "{tool}: {reply}");
    }
    assert!(replies[0].contains(BOOT), "{}", replies[0]);
}

#[test]
fn doctor_names_both_grammars_and_version_the_store_schema() {
    let tree = Tree::fixture("go-basic");
    let doctor = tree.ok(&["doctor"]);
    let grammar = doctor["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == "grammar")
        .expect("grammar check");
    let detail = text(&grammar["detail"]);
    assert!(detail.starts_with("tree-sitter-rust 0."), "{detail}");
    assert!(detail.ends_with(", tree-sitter-go 0.25.0"), "{detail}");
    assert_eq!(tree.ok(&["version"])["store_schema"], 5);
}

/// `find` keeps only files the walk `index` uses still keeps: once a tree
/// holds a `go.mod`, its `vendor/` is left out, whatever language is there.
#[test]
fn find_walks_a_tree_as_index_does() {
    let tree = Tree::empty("go-vendor");
    fs::create_dir_all(tree.repo.join("vendor/v")).expect("vendor");
    fs::write(tree.repo.join("vendor/v/v.rs"), "pub fn vendored() {}\n").expect("v.rs");
    tree.ok(&["index", "."]);
    assert_eq!(
        column(&tree.ok(&["find", "vendored"])["hits"], "path"),
        ["vendor/v/v.rs"]
    );
    fs::write(tree.repo.join("go.mod"), "module example.com/m\n").expect("go.mod");
    assert!(column(&tree.ok(&["find", "vendored"])["hits"], "path").is_empty());
}
