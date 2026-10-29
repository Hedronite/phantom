//! `--max-tokens` and `max_tokens` on `find`, `show`, `callers`,
//! `callees`, `impact`, `rank` and `cochange`, and the `focus` cap
//! (SPEC-w8 G0c, G3a).
//!
//! Every budgeted reply is checked byte for byte against a reply built
//! here from the unbudgeted one: its entries cut out of the printed text
//! in the SPEC's drop order, one at a time, until the estimate fits, and
//! `tokens` spliced in with `used` settled on the estimate of that very
//! line. The estimate and the packing budget are computed here too, from
//! their definitions, not from the analyze crate.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-max-tokens-{label}-{}-{}",
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

/// An indexed tree and its data directory.
struct World {
    tree: TempDir,
    /// A git root, when the world is one; it is then the tree.
    repo: Option<support::Repo>,
    data: TempDir,
}

impl World {
    fn fixture(name: &str) -> Self {
        let world = Self {
            tree: TempDir::new("tree"),
            repo: None,
            data: TempDir::new("data"),
        };
        copy_tree(&Path::new(FIXTURES).join(name), world.tree.path());
        world.index();
        world
    }

    /// A package with many entries for every op to drop: 30 modules, each
    /// with a `dup`, a direct caller of `hub` and two callers above it, a
    /// test per module, a struct with 30 methods, a `fan` that calls every
    /// `dup`, and a `relay` that calls every `dup` and that every top
    /// caller calls.
    fn generated() -> Self {
        let world = Self {
            tree: TempDir::new("tree"),
            repo: None,
            data: TempDir::new("data"),
        };
        let root = world.tree.path();
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let mut lib = String::new();
        let mut methods = String::new();
        let mut fan = String::new();
        let mut tests = String::new();
        for i in 0..30 {
            lib.push_str(&format!("pub mod m{i};\n"));
            methods.push_str(&format!("    pub fn method_{i}(&self) {{}}\n"));
            fan.push_str(&format!("    crate::m{i}::dup();\n"));
            tests.push_str(&format!(
                "    #[test]\n    fn t{i}() {{\n        crate::m{i}::caller_{i}();\n    }}\n"
            ));
            fs::write(
                root.join(format!("src/m{i}.rs")),
                format!(
                    "/// Module {i}'s dup.\npub fn dup() {{}}\n\npub fn caller_{i}() {{\n    \
                     crate::hub();\n}}\n\npub fn deep_{i}() {{\n    caller_{i}();\n}}\n\n\
                     pub fn deeper_{i}() {{\n    deep_{i}();\n    crate::relay();\n}}\n"
                ),
            )
            .expect("write module");
        }
        lib.push_str(&format!(
            "\n/// Many methods.\npub struct Hub;\n\nimpl Hub {{\n{methods}}}\n\n\
             /// Called from every module.\npub fn hub() {{}}\n\n\
             /// Calls every dup.\npub fn fan() {{\n{fan}}}\n\n\
             /// Called by every top caller, and calls every dup.\npub fn relay() {{\n{fan}}}\n\n\
             #[cfg(test)]\nmod tests {{\n{tests}}}\n"
        ));
        fs::write(root.join("src/lib.rs"), lib).expect("write lib");
        world.index();
        world
    }

    /// A git root with the support history, indexed with its co-change.
    fn history() -> Self {
        let world = Self {
            tree: TempDir::new("unused"),
            repo: Some(support::Repo::new("budget", &support::history())),
            data: TempDir::new("data"),
        };
        world.index();
        world
    }

    fn root(&self) -> &Path {
        self.repo
            .as_ref()
            .map_or_else(|| self.tree.path(), |repo| repo.root.as_path())
    }

    fn index(&self) {
        let (code, line) = self.cli(&["index", "."]);
        assert_eq!(code, Some(0), "{line}");
    }

    /// The exit code and the one stdout line, newline removed.
    fn cli(&self, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(BIN)
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

    /// The text item of each tool call, from one server rooted at the tree.
    fn mcp(&self, calls: &[(&str, Value)]) -> Vec<String> {
        let mut input = String::from(
            "{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"budget\",\"version\":\"0\"}}}\n",
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
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        let replies: Vec<Value> = stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect();
        assert_eq!(replies.len(), calls.len() + 1);
        replies[1..]
            .iter()
            .map(|reply| {
                reply["result"]["content"][0]["text"]
                    .as_str()
                    .expect("text")
                    .to_owned()
            })
            .collect()
    }
}

/// `ceil(bytes / 3.5) + 1` for one line with no newline.
fn estimate(line: &str) -> u64 {
    (line.len() as f64 / 3.5).ceil() as u64 + 1
}

/// 95% of `max`, rounded down.
fn room(max: u64) -> u64 {
    max * 95 / 100
}

/// One past the last byte of the JSON value that starts at `at`.
fn value_end(raw: &[u8], at: usize) -> usize {
    match raw[at] {
        b'"' => {
            let mut i = at + 1;
            while raw[i] != b'"' {
                i += if raw[i] == b'\\' { 2 } else { 1 };
            }
            i + 1
        }
        b'{' | b'[' => {
            let mut depth = 0;
            let mut i = at;
            loop {
                match raw[i] {
                    b'"' => {
                        i = value_end(raw, i);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return i + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        _ => {
            let mut i = at;
            while !matches!(raw[i], b',' | b'}' | b']') {
                i += 1;
            }
            i
        }
    }
}

/// The members of the object at `at`, as printed: each key and the span
/// of its value.
fn members(raw: &[u8], at: usize) -> Vec<(String, usize, usize)> {
    assert_eq!(raw[at], b'{');
    let mut out = Vec::new();
    let mut i = at + 1;
    while raw[i] != b'}' {
        let key_end = value_end(raw, i);
        let key = String::from_utf8(raw[i + 1..key_end - 1].to_vec()).expect("key");
        let start = key_end + 1;
        let end = value_end(raw, start);
        out.push((key, start, end));
        i = if raw[end] == b',' { end + 1 } else { end };
    }
    out
}

/// The span of the value of `key` in the object at `at`.
fn member(raw: &[u8], at: usize, key: &str) -> Option<(usize, usize)> {
    members(raw, at)
        .into_iter()
        .find(|(name, _, _)| name == key)
        .map(|(_, start, end)| (start, end))
}

/// The spans of the elements of the array at `at`.
fn elements(raw: &[u8], at: usize) -> Vec<(usize, usize)> {
    assert_eq!(raw[at], b'[');
    let mut out = Vec::new();
    let mut i = at + 1;
    while raw[i] != b']' {
        let end = value_end(raw, i);
        out.push((i, end));
        i = if raw[end] == b',' { end + 1 } else { end };
    }
    out
}

/// The bytes each drop removes from the array at `at`, last element
/// first: the element and the comma before it, or the first element alone.
fn array_drops(raw: &[u8], at: usize) -> Vec<(usize, usize)> {
    let spans = elements(raw, at);
    (0..spans.len())
        .rev()
        .map(|j| {
            if j == 0 {
                spans[0]
            } else {
                (spans[j - 1].1, spans[j].1)
            }
        })
        .collect()
}

/// The SPEC-w8 G0c drop order for `op`, as byte ranges of `line`.
fn drops(op: &str, line: &str) -> Vec<(usize, usize)> {
    let raw = line.as_bytes();
    let (data, _) = member(raw, 0, "data").expect("data");
    let array = |object: usize, key: &str| {
        member(raw, object, key).map_or_else(Vec::new, |(start, _)| array_drops(raw, start))
    };
    match op {
        "find" | "callers" | "callees" | "rank" => array(data, "hits"),
        "cochange" => array(data, "partners"),
        "impact" => {
            let mut out = Vec::new();
            if let Some((groups, _)) = member(raw, data, "groups") {
                let mut by_depth: Vec<(u64, usize)> = elements(raw, groups)
                    .into_iter()
                    .map(|(start, _)| {
                        let (depth, end) = member(raw, start, "depth").expect("depth");
                        (line[depth..end].parse().expect("depth"), start)
                    })
                    .collect();
                by_depth.sort_by_key(|&(depth, _)| std::cmp::Reverse(depth));
                for (_, group) in by_depth {
                    out.extend(array(group, "hits"));
                }
            }
            out.extend(array(data, "tests"));
            out
        }
        "show" => {
            let mut out = Vec::new();
            let fields = members(raw, data);
            if let Some(at) = fields.iter().position(|(key, _, _)| key == "source") {
                out.push((fields[at - 1].2, fields[at].2));
            }
            if let Some((start, _)) = member(raw, data, "cochange") {
                out.extend(array_drops(raw, start));
            }
            for side in ["callees", "callers", "members"] {
                let (start, _) = member(raw, data, side).expect("side");
                out.extend(array(start, "hits"));
            }
            out
        }
        _ => unreachable!("{op}"),
    }
}

/// `line` with the first `count` drops cut out and `tokens` added at the
/// end of `data`, with `used` settled on the estimate of the result.
fn trimmed(line: &str, drops: &[(usize, usize)], count: usize, max: u64) -> String {
    let raw = line.as_bytes();
    let (_, data_end) = member(raw, 0, "data").expect("data");
    let mut cut: Vec<(usize, usize)> = drops[..count].to_vec();
    cut.sort_unstable();
    let mut kept = String::new();
    let mut cursor = 0;
    for (start, end) in cut {
        kept.push_str(&line[cursor..start]);
        cursor = end;
    }
    kept.push_str(&line[cursor..data_end - 1]);
    let mut used = 0;
    loop {
        let candidate = format!(
            "{kept},\"tokens\":{{\"max\":{max},\"used\":{used},\"omitted\":{count}}}{}",
            &line[data_end - 1..]
        );
        let cost = estimate(&candidate);
        if cost == used {
            return candidate;
        }
        used = cost;
    }
}

/// What a budgeted run must print: the reply after the fewest drops that
/// fit, or none when nothing fits.
fn expected(op: &str, line: &str, max: u64) -> Option<String> {
    let drops = drops(op, line);
    (0..=drops.len())
        .map(|count| trimmed(line, &drops, count, max))
        .find(|reply| estimate(reply) <= room(max))
}

/// One read and the tool call that must print the same.
struct Case {
    op: &'static str,
    cli: Vec<&'static str>,
    tool: &'static str,
    arguments: Value,
}

fn case(op: &'static str, cli: &[&'static str], tool: &'static str, arguments: Value) -> Case {
    Case {
        op,
        cli: cli.to_vec(),
        tool,
        arguments,
    }
}

/// At least 20 budgets from 1 to past what the whole reply costs, plus the
/// largest.
fn budgets(full: &str) -> Vec<u64> {
    let top = (estimate(full) + 20) * 100 / 95 + 2;
    let mut out: Vec<u64> = (0..24).map(|i| 1 + (top - 1) * i / 23).collect();
    out.push(100_000);
    out.dedup();
    out
}

/// Checks every case at every budget, in the CLI and over MCP, and returns
/// how many replies dropped something and how many were too small.
fn check(world: &World, cases: &[Case]) -> (usize, usize) {
    let (mut dropped, mut small) = (0, 0);
    let mut calls = Vec::new();
    let mut lines = Vec::new();
    for case in cases {
        let (code, full) = world.cli(&case.cli);
        assert_eq!(code, Some(0), "{full}");
        assert!(!full.contains("\"tokens\""), "{full}");
        for max in budgets(&full) {
            let text = max.to_string();
            let mut args = case.cli.clone();
            args.extend(["--max-tokens", &text]);
            let (code, line) = world.cli(&args);
            match expected(case.op, &full, max) {
                Some(want) => {
                    assert_eq!(code, Some(0), "{args:?}");
                    assert_eq!(line, want, "{args:?}");
                    let envelope: Value = serde_json::from_str(&line).expect("envelope");
                    let tokens = &envelope["data"]["tokens"];
                    assert_eq!(tokens["used"], estimate(&line), "{args:?}");
                    assert!(estimate(&line) <= room(max), "{args:?}");
                    if tokens["omitted"] != 0 {
                        dropped += 1;
                    }
                }
                None => {
                    assert_eq!(code, Some(1), "{args:?}");
                    let envelope: Value = serde_json::from_str(&line).expect("envelope");
                    assert_eq!(envelope["error"]["kind"], "usage", "{line}");
                    let message = envelope["error"]["message"].as_str().expect("message");
                    assert!(message.starts_with("budget_too_small: "), "{message}");
                    small += 1;
                }
            }
            let mut arguments = case.arguments.clone();
            arguments["max_tokens"] = json!(max);
            calls.push((case.tool, arguments));
            lines.push(line);
        }
    }
    assert_eq!(world.mcp(&calls), lines, "MCP equals the CLI");
    (dropped, small)
}

#[test]
fn every_budgeted_reply_is_the_fewest_drops_that_fit() {
    let world = World::generated();
    let cases = [
        case(
            "find",
            &["find", "dup"],
            "find_symbols",
            json!({ "query": "dup" }),
        ),
        case(
            "callers",
            &["callers", "hub"],
            "callers",
            json!({ "symbol": "hub" }),
        ),
        case(
            "callees",
            &["callees", "fan"],
            "callees",
            json!({ "symbol": "fan" }),
        ),
        case(
            "impact",
            &["impact", "hub"],
            "impact",
            json!({ "symbol": "hub" }),
        ),
        case(
            "impact",
            &["impact", "hub", "--no-tests", "--depth", "2"],
            "impact",
            json!({ "symbol": "hub", "include_tests": false, "depth": 2 }),
        ),
        case(
            "impact",
            &["impact", "hub", "--summary"],
            "impact",
            json!({ "symbol": "hub", "summary": true }),
        ),
        case(
            "show",
            &["show", "Hub"],
            "show_symbol",
            json!({ "symbol": "Hub" }),
        ),
        case(
            "show",
            &["show", "hub", "--source"],
            "show_symbol",
            json!({ "symbol": "hub", "source": true }),
        ),
        case(
            "show",
            &["show", "fan", "--source"],
            "show_symbol",
            json!({ "symbol": "fan", "source": true }),
        ),
        case(
            "show",
            &["show", "relay", "--source"],
            "show_symbol",
            json!({ "symbol": "relay", "source": true }),
        ),
        case(
            "rank",
            &["rank", "--limit", "100"],
            "top_ranked",
            json!({ "limit": 100 }),
        ),
        case(
            "rank",
            &["rank", "--files", "--focus", "src/m3.rs"],
            "top_ranked",
            json!({ "kind": "files", "focus": ["src/m3.rs"] }),
        ),
    ];
    let (dropped, small) = check(&world, &cases);
    assert!(dropped >= 100, "{dropped} replies dropped entries");
    assert!(small >= cases.len(), "{small} replies were too small");
}

#[test]
fn fixtures_fit_the_same_way() {
    let world = World::fixture("impact");
    let cases = [
        case(
            "find",
            &["find", "helper"],
            "find_symbols",
            json!({ "query": "helper" }),
        ),
        case(
            "callers",
            &["callers", "root_fn"],
            "callers",
            json!({ "symbol": "root_fn" }),
        ),
        case(
            "callees",
            &["callees", "run"],
            "callees",
            json!({ "symbol": "run" }),
        ),
        case(
            "impact",
            &["impact", "root_fn"],
            "impact",
            json!({ "symbol": "root_fn" }),
        ),
        case(
            "show",
            &["show", "area", "--source"],
            "show_symbol",
            json!({ "symbol": "area", "source": true }),
        ),
        case("rank", &["rank"], "top_ranked", json!({})),
    ];
    check(&world, &cases);
}

#[test]
fn cochange_and_show_on_a_git_root_fit_the_same_way() {
    let world = World::history();
    let cases = [
        case(
            "cochange",
            &["cochange", "src/a.rs", "--min-lift", "0"],
            "cochange",
            json!({ "file": "src/a.rs", "min_lift": 0 }),
        ),
        case(
            "cochange",
            &["cochange", "src/d.rs"],
            "cochange",
            json!({ "file": "src/d.rs" }),
        ),
        case(
            "show",
            &["show", "fa"],
            "show_symbol",
            json!({ "symbol": "fa" }),
        ),
        case(
            "show",
            &["show", "fa", "--source"],
            "show_symbol",
            json!({ "symbol": "fa", "source": true }),
        ),
    ];
    let (dropped, small) = check(&world, &cases);
    assert!(dropped >= 20, "{dropped} replies dropped entries");
    assert!(small >= cases.len(), "{small} replies were too small");
}

#[test]
fn a_budget_changes_nothing_else() {
    let world = World::generated();
    // A budget the whole reply fits in drops nothing, and taking `tokens`
    // back out leaves the unbudgeted line.
    let (_, full) = world.cli(&["impact", "hub"]);
    let (_, fitted) = world.cli(&["impact", "hub", "--max-tokens", "100000"]);
    let start = fitted.find(",\"tokens\":").expect("tokens");
    let end = start + fitted[start..].find('}').expect("end") + 1;
    assert_eq!(format!("{}{}", &fitted[..start], &fitted[end..]), full);
    assert!(fitted[start..end].contains("\"omitted\":0"), "{fitted}");
    // Errors carry no tokens, and the same run twice prints the same.
    let (code, missing) = world.cli(&["callers", "nothing_here", "--max-tokens", "50"]);
    assert_eq!(code, Some(3), "{missing}");
    assert_eq!(missing, world.cli(&["callers", "nothing_here"]).1);
    let again = world.cli(&["show", "Hub", "--max-tokens", "300"]);
    assert_eq!(again, world.cli(&["show", "Hub", "--max-tokens", "300"]));
}

#[test]
fn max_tokens_is_1_to_100000_on_exactly_six_commands() {
    let world = World::generated();
    for args in [
        ["find", "dup"],
        ["show", "Hub"],
        ["callers", "hub"],
        ["callees", "fan"],
        ["impact", "hub"],
        ["rank", "--files"],
    ] {
        for bad in ["0", "100001", "-1", "2.5", "x"] {
            let output = Command::new(BIN)
                .args(args)
                .args(["--max-tokens", bad])
                .current_dir(world.root())
                .env("PHANTOM_FRACTAL_DATA", world.data.path())
                .output()
                .expect("run");
            assert_eq!(output.status.code(), Some(1), "{args:?} {bad}");
            assert!(output.stdout.is_empty(), "{args:?} {bad}");
        }
        for good in ["1", "100000"] {
            let (code, line) = world.cli(&[args[0], args[1], "--max-tokens", good]);
            assert!(code == Some(0) || code == Some(1), "{line}");
        }
    }
    for args in [
        &["map", "--budget", "500", "--max-tokens", "500"][..],
        &["index", ".", "--max-tokens", "500"][..],
        &["version", "--max-tokens", "500"][..],
    ] {
        let (code, line) = world.cli(args);
        assert_eq!((code, line.as_str()), (Some(1), ""), "{args:?}");
    }
    let tools = [
        ("find_symbols", json!({ "query": "dup" })),
        ("show_symbol", json!({ "symbol": "Hub" })),
        ("callers", json!({ "symbol": "hub" })),
        ("callees", json!({ "symbol": "fan" })),
        ("impact", json!({ "symbol": "hub" })),
        ("top_ranked", json!({})),
    ];
    let mut calls = Vec::new();
    for (tool, arguments) in &tools {
        for bad in [
            json!(0),
            json!(100_001),
            json!(2.5),
            json!("5"),
            json!(null),
        ] {
            let mut arguments = arguments.clone();
            arguments["max_tokens"] = bad;
            calls.push((*tool, arguments));
        }
    }
    let replies = world.mcp(&calls);
    for reply in &replies {
        let envelope: Value = serde_json::from_str(reply).expect("envelope");
        assert_eq!(
            envelope["error"],
            json!({
                "kind": "usage",
                "message": "max_tokens must be an integer from 1 to 100000",
                "exit": 1,
            }),
            "{reply}"
        );
    }
}

#[test]
fn focus_takes_at_most_100_paths() {
    let world = World::generated();
    let refused = json!({
        "kind": "usage",
        "message": "focus takes at most 100 paths",
        "exit": 1,
    });
    for (prefix, tool, base) in [
        (&["rank"][..], "top_ranked", json!({})),
        (
            &["map", "--budget", "500"][..],
            "repo_map",
            json!({ "budget": 500 }),
        ),
    ] {
        for (count, ok) in [(100, true), (101, false), (150, false)] {
            let mut args: Vec<&str> = prefix.to_vec();
            for _ in 0..count {
                args.extend(["--focus", "src/m3.rs"]);
            }
            let (code, line) = world.cli(&args);
            let envelope: Value = serde_json::from_str(&line).expect("envelope");
            if ok {
                assert_eq!(code, Some(0), "{line}");
                assert_eq!(envelope["ok"], true);
            } else {
                assert_eq!(code, Some(1), "{line}");
                assert_eq!(envelope["error"], refused);
            }
            let mut arguments = base.clone();
            arguments["focus"] = json!(vec!["src/m3.rs"; count]);
            assert_eq!(world.mcp(&[(tool, arguments)]), [line], "{tool} {count}");
        }
    }
    // The cap comes before the index is opened, as the limit does.
    let empty = TempDir::new("empty");
    let mut args = vec!["rank", "--repo"];
    let path = empty.path().to_str().expect("utf-8");
    args.push(path);
    for _ in 0..101 {
        args.extend(["--focus", "a.rs"]);
    }
    let (code, line) = world.cli(&args);
    assert_eq!(code, Some(1), "{line}");
}
