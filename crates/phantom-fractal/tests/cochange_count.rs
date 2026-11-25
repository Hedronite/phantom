//! `cochange --count` and MCP `mode: "count"` (SPEC-w14 G3b), on a
//! synthetic git root from `support`.
//!
//! The counts are checked against a count made here from `git log`, one
//! per kept commit, for every tracked file; the test runs `git`, Phantom
//! never does.

mod support;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use support::{DAY, HEAD, Repo, Script, TempDir, database, history, module, snapshot};

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

/// The command line's exit code and stdout line, newline removed.
fn cli(dir: &Path, data: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", data)
        .output()
        .expect("run cli");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    (
        output.status.code(),
        stdout.strip_suffix('\n').unwrap_or(&stdout).to_owned(),
    )
}

fn data(dir: &Path, store: &Path, args: &[&str]) -> Value {
    let (code, line) = cli(dir, store, args);
    assert_eq!(code, Some(0), "{args:?}: {line}");
    serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone()
}

/// `history()` plus one commit that adds `README.md` beside a change to
/// `a`, and one bulk commit of 31 files that also changes `a`.
fn world() -> (Repo, TempDir) {
    let mut more = Script::after_main();
    let (a, body) = module("a", 100);
    more.commit(
        HEAD + DAY,
        &[("README.md", "notes\n".to_owned()), (a.as_str(), body)],
    );
    let mut bulk: Vec<(String, String)> = (0..30)
        .map(|n| (format!("docs/page{n:02}.md"), format!("page {n}\n")))
        .collect();
    bulk.push(module("a", 101));
    let borrowed: Vec<(&str, String)> = bulk
        .iter()
        .map(|(path, body)| (path.as_str(), body.clone()))
        .collect();
    more.commit(HEAD + 2 * DAY, &borrowed);
    let repo = Repo::new("count", &history());
    repo.import(&more);
    let store = TempDir::new("count-data");
    data(&repo.root, store.path(), &["index", "."]);
    (repo, store)
}

/// Every tracked file's partners of `asked`, counted from `git log`: one
/// per non-merge commit of at most 30 files that changes both.
fn oracle(repo: &Repo, asked: &str) -> BTreeMap<String, u64> {
    let log = repo.git(
        &["log", "--no-merges", "--format=format:@", "--name-only"],
        None,
    );
    let mut counts = BTreeMap::new();
    for commit in log
        .split('@')
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        let files: Vec<&str> = commit.lines().filter(|line| !line.is_empty()).collect();
        if files.len() > 30 || !files.contains(&asked) {
            continue;
        }
        for file in files.into_iter().filter(|file| *file != asked) {
            *counts.entry(file.to_owned()).or_insert(0) += 1;
        }
    }
    counts
}

fn partners(data: &Value) -> BTreeMap<String, u64> {
    data["partners"]
        .as_array()
        .expect("partners")
        .iter()
        .map(|partner| {
            (
                partner["path"].as_str().expect("path").to_owned(),
                partner["count"].as_u64().expect("count"),
            )
        })
        .collect()
}

#[test]
fn count_lists_every_tracked_partner_with_its_shared_commits() {
    let (repo, store) = world();
    let data = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count"],
    );
    assert_eq!(data["file"], "src/a.rs");
    assert_eq!(data["mode"], "count");
    assert_eq!(
        data["scope"],
        json!({ "files": "tracked", "weighting": "none", "min_commits": 1, "bulk_skip": 30 })
    );
    assert_eq!(
        data["scan"],
        json!({
            "head": repo.rev("HEAD"),
            "window": { "days": 365, "max_commits": 5000 },
            "commits_seen": 25,
            "skipped_bulk": 1,
        })
    );
    assert_eq!(
        data["partners"],
        json!([
            { "path": "src/b.rs", "count": 6 },
            { "path": "src/c.rs", "count": 5 },
            { "path": "src/g.rs", "count": 4 },
            { "path": "Cargo.toml", "count": 1 },
            { "path": "README.md", "count": 1 },
            { "path": "src/d.rs", "count": 1 },
            { "path": "src/e.rs", "count": 1 },
            { "path": "src/f.rs", "count": 1 },
            { "path": "src/lib.rs", "count": 1 },
        ])
    );
    assert_eq!(data["truncated"], false);
    let keys: Vec<&str> = data
        .as_object()
        .expect("data")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 6, "{data}");
}

#[test]
fn count_agrees_with_git_log_for_every_tracked_file() {
    let (repo, store) = world();
    let tracked = repo.git(&["ls-files"], None);
    for file in tracked.lines() {
        let data = data(
            &repo.root,
            store.path(),
            &["cochange", file, "--count", "--limit", "100"],
        );
        assert_eq!(partners(&data), oracle(&repo, file), "{file}");
        let counts: Vec<u64> = data["partners"]
            .as_array()
            .expect("partners")
            .iter()
            .map(|partner| partner["count"].as_u64().expect("count"))
            .collect();
        assert!(
            counts.windows(2).all(|pair| pair[0] >= pair[1]),
            "{file}: {counts:?}"
        );
    }
}

#[test]
fn count_takes_a_limit_and_a_budget_and_writes_nothing() {
    let (repo, store) = world();
    let tree = snapshot(&repo.root);
    let db = database(store.path());
    let full = data(
        &repo.root,
        store.path(),
        &["cochange", "./src/a.rs", "--count"],
    );
    assert_eq!(full["file"], "src/a.rs");
    let two = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count", "--limit", "2"],
    );
    assert_eq!(
        two["partners"],
        json!(full["partners"].as_array().expect("partners")[..2])
    );
    assert_eq!(two["truncated"], true);
    // `src/a.rs` has 9 partners: a limit of 9 lists them all, untruncated.
    let nine = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count", "--limit", "9"],
    );
    assert_eq!(nine["partners"], full["partners"]);
    assert_eq!(nine["truncated"], false);
    let eight = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count", "--limit", "8"],
    );
    assert_eq!(eight["truncated"], true);
    // A budget just under what the whole reply costs drops some partners.
    let (_, line) = cli(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count"],
    );
    let whole = (line.len() as u64 * 2).div_ceil(7) + 1;
    let budget = (whole * 100 / 95).to_string();
    let fitted = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count", "--max-tokens", &budget],
    );
    let kept = fitted["partners"].as_array().expect("partners").len();
    let all = full["partners"].as_array().expect("partners");
    assert!(kept < all.len(), "{fitted}");
    assert_eq!(fitted["partners"], json!(all[..kept]));
    assert_eq!(fitted["tokens"]["omitted"], all.len() - kept);
    assert_eq!(snapshot(&repo.root), tree, "count wrote the tree");
    assert_eq!(database(store.path()), db, "count wrote the index");
}

#[test]
fn count_and_min_lift_and_bad_values_are_usage() {
    let (repo, store) = world();
    for args in [
        &["cochange", "src/a.rs", "--count", "--min-lift", "2"][..],
        &["cochange", "src/a.rs", "--count", "--limit", "0"][..],
        &["cochange", "src/a.rs", "--count", "--max-tokens", "0"][..],
    ] {
        assert_eq!(
            cli(&repo.root, store.path(), args),
            (Some(1), String::new()),
            "{args:?}"
        );
    }
}

#[test]
fn a_file_not_indexed_is_not_found_in_the_decayed_mode_and_counted_in_count() {
    let (repo, store) = world();
    for file in ["README.md", "Cargo.toml", "src/gone.rs"] {
        let (code, line) = cli(&repo.root, store.path(), &["cochange", file]);
        assert_eq!(code, Some(3), "{line}");
        let envelope: Value = serde_json::from_str(&line).expect("envelope");
        assert_eq!(envelope["error"]["kind"], "not_found", "{line}");
        let message = envelope["error"]["message"].as_str().expect("message");
        assert!(
            message.starts_with("file is not indexed at HEAD"),
            "{message}"
        );
        assert!(
            message.contains("cochange --count") && message.contains("git log"),
            "{message}"
        );
        assert!(!message.contains(file), "{message}");
    }
    let readme = data(
        &repo.root,
        store.path(),
        &["cochange", "README.md", "--count"],
    );
    assert_eq!(
        readme["partners"],
        json!([{ "path": "src/a.rs", "count": 1 }])
    );
    // A path that was never tracked is not_found in count mode too (A8).
    let (code, line) = cli(
        &repo.root,
        store.path(),
        &["cochange", "src/gone.rs", "--count"],
    );
    assert_eq!((code, line), (Some(3), not_tracked("src/gone.rs")));
}

/// The `not_found` line for a file no tree at HEAD holds, naming it as
/// `shown`.
fn not_tracked(shown: &str) -> String {
    format!(
        "{{\"ok\":false,\"data\":null,\"error\":{{\"kind\":\"not_found\",\"message\":\
         \"file is not tracked at HEAD, so it has no co-change count; for its history use \
         git log -- {shown}\",\"exit\":3}},\"schema\":1}}"
    )
}

/// `history()`, then a commit adding `docs/old.md` beside a change to `a`,
/// then one deleting `docs/old.md`.
fn deleted() -> (Repo, TempDir) {
    let mut more = Script::after_main();
    let (a, body) = module("a", 200);
    more.commit(
        HEAD + DAY,
        &[("docs/old.md", "old\n".to_owned()), (a.as_str(), body)],
    );
    more.delete(HEAD + 2 * DAY, &["docs/old.md"]);
    let repo = Repo::new("count-deleted", &history());
    repo.import(&more);
    let store = TempDir::new("count-deleted-data");
    data(&repo.root, store.path(), &["index", "."]);
    (repo, store)
}

#[test]
fn a_deleted_or_never_tracked_file_is_not_found_and_points_to_git_log() {
    let (repo, store) = deleted();
    assert!(!repo.root.join("docs/old.md").exists());
    let absolute = repo.root.join("src/a.rs");
    let absolute = absolute.to_str().expect("utf-8");
    for (file, shown) in [
        ("docs/old.md", "docs/old.md"),
        ("./docs/old.md", "docs/old.md"),
        ("src/never.rs", "src/never.rs"),
        ("docs", "docs"),
        (absolute, "FILE"),
    ] {
        let (code, line) = cli(&repo.root, store.path(), &["cochange", file, "--count"]);
        assert_eq!(
            (code, line.as_str()),
            (Some(3), not_tracked(shown).as_str()),
            "{file}"
        );
        assert!(!line.contains(repo.root.to_str().expect("utf-8")), "{line}");
    }
    let texts = serve(
        &repo.root,
        store.path(),
        &[
            json!({ "file": "docs/old.md", "mode": "count" }),
            json!({ "file": "src/never.rs", "mode": "count" }),
        ],
    );
    assert_eq!(
        texts,
        [not_tracked("docs/old.md"), not_tracked("src/never.rs")]
    );
    // A tracked file is still counted.
    let counted = data(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count"],
    );
    assert_eq!(
        counted["partners"][0],
        json!({ "path": "src/b.rs", "count": 6 })
    );
}

#[test]
fn a_tree_without_git_has_no_scan_to_count() {
    let tree = TempDir::new("count-nogit");
    let store = TempDir::new("count-nogit-data");
    support::copy_fixture("graph", tree.path());
    data(tree.path(), store.path(), &["index", "."]);
    let data = data(
        tree.path(),
        store.path(),
        &["cochange", "app/src/dup.rs", "--count"],
    );
    assert_eq!(data["scan"], Value::Null);
    assert_eq!(data["partners"], json!([]));
    assert_eq!(data["mode"], "count");
}

/// The text item of each `cochange` call, in one MCP session.
fn serve(dir: &Path, store: &Path, calls: &[Value]) -> Vec<String> {
    let mut input = String::from(
        "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"count\",\"version\":\"0\"}}}\n",
    );
    for (id, arguments) in (1..).zip(calls) {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": "cochange", "arguments": arguments },
        });
        input.push_str(&format!("{message}\n"));
    }
    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(dir)
        .env("PHANTOM_FRACTAL_DATA", store)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .skip(1)
        .map(|line| {
            let reply: Value = serde_json::from_str(line).expect("JSON line");
            reply["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .to_owned()
        })
        .collect()
}

#[test]
fn mode_count_over_mcp_prints_the_command_line_reply() {
    let (repo, store) = world();
    let texts = serve(
        &repo.root,
        store.path(),
        &[
            json!({ "file": "src/a.rs", "mode": "count", "limit": 3 }),
            json!({ "file": "src/a.rs", "mode": "decayed" }),
            json!({ "file": "src/a.rs" }),
            json!({ "file": "src/a.rs", "mode": "count", "min_lift": 2 }),
            json!({ "file": "src/a.rs", "mode": "raw" }),
        ],
    );
    let (_, count) = cli(
        &repo.root,
        store.path(),
        &["cochange", "src/a.rs", "--count", "--limit", "3"],
    );
    let (_, decayed) = cli(&repo.root, store.path(), &["cochange", "src/a.rs"]);
    assert_eq!(texts[0], count);
    assert_eq!(texts[1], decayed);
    assert_eq!(texts[2], decayed);
    for (text, message) in [
        (&texts[3], "min_lift is for mode decayed only"),
        (&texts[4], "mode must be decayed or count"),
    ] {
        let envelope: Value = serde_json::from_str(text).expect("envelope");
        assert_eq!(envelope["error"]["kind"], "usage", "{text}");
        assert_eq!(envelope["error"]["message"], message, "{text}");
    }
}

/// The README line that starts with `prefix`.
fn readme_line(prefix: &str) -> String {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
        .expect("README");
    readme
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no README line starts {prefix}"))
        .to_owned()
}

#[test]
fn the_readme_examples_are_the_replies() {
    let repo = Repo::new("readme", &history());
    let store = TempDir::new("readme-data");
    data(&repo.root, store.path(), &["index", "."]);
    for (args, prefix) in [
        (
            &["cochange", "src/a.rs"][..],
            "{\"ok\":true,\"data\":{\"file\":\"src/a.rs\",\"scan\"",
        ),
        (
            &["cochange", "src/a.rs", "--count"][..],
            "{\"ok\":true,\"data\":{\"file\":\"src/a.rs\",\"mode\":\"count\"",
        ),
    ] {
        let (code, line) = cli(&repo.root, store.path(), args);
        assert_eq!(code, Some(0), "{line}");
        assert_eq!(readme_line(prefix), line, "{args:?}");
    }
}
