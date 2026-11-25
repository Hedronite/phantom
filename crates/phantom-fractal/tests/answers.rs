//! Answers agents can use (SPEC-w14 G1b): `find` over methods, a missed
//! lookup's `did_you_mean`, and the `display`, `callers_distinct`,
//! `files` and `sites` keys of `callers`, `callees` and `impact`, which
//! sit after the old keys and go first under a budget. And SPEC-w16 G1b,
//! on the command line and over MCP: the crate-name and `path:line` forms,
//! the `retry` of a miss, and a limit above 100.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

mod support;

use support::{Repo, TempDir, copy_fixture, history, raw_keys};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

/// `fixtures/resolve`, copied and indexed in its own data directory.
struct World {
    repo: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            repo: TempDir::new("answers-repo"),
            data: TempDir::new("answers-data"),
        };
        copy_fixture("resolve", world.repo.path());
        let (code, line) = world.line(BINS[0], &["index", "."]);
        assert_eq!(code, Some(0), "{line}");
        world
    }

    /// The exit code and the printed line, without its newline.
    fn line(&self, bin: &str, args: &[&str]) -> (Option<i32>, String) {
        let out = Command::new(bin)
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .current_dir(self.repo.path())
            .output()
            .expect("run");
        let text = String::from_utf8(out.stdout).expect("utf-8");
        (out.status.code(), text.trim_end().to_owned())
    }

    fn run(&self, bin: &str, args: &[&str]) -> (Option<i32>, Value) {
        let (code, line) = self.line(bin, args);
        (code, serde_json::from_str(&line).expect("envelope"))
    }

    fn data(&self, bin: &str, args: &[&str]) -> Value {
        let (code, env) = self.run(bin, args);
        assert_eq!(code, Some(0), "{args:?}: {env}");
        env["data"].clone()
    }
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("list")
        .iter()
        .map(|item| item["name"].as_str().expect("name"))
        .collect()
}

#[test]
fn find_returns_a_method_by_its_bare_name_qualified_by_its_type() {
    let world = World::new();
    for bin in BINS {
        let data = world.data(bin, &["find", "bump"]);
        assert_eq!(
            data["hits"],
            json!([{
                "name": "crate::Counter::bump",
                "kind": "method",
                "visibility": "pub",
                "path": "src/lib.rs",
                "line": 51,
            }]),
            "{bin}: {data}"
        );
        let greet = world.data(bin, &["find", "greet"]);
        assert!(
            names(&greet["hits"]).contains(&"crate::tricky::Guest::greet"),
            "{bin}: {greet}"
        );
    }
}

#[test]
fn find_keeps_an_item_hit_bare_with_its_old_keys_in_order() {
    let world = World::new();
    let (code, line) = world.line(BINS[0], &["find", "helper"]);
    assert_eq!(code, Some(0), "{line}");
    let data: Value = serde_json::from_str::<Value>(&line).expect("envelope")["data"].clone();
    let hits = data["hits"].as_array().expect("hits");
    assert!(hits.len() >= 2, "{data}");
    for hit in hits {
        assert_eq!(hit["name"], "helper", "{hit}");
        assert_eq!(hit["kind"], "function", "{hit}");
    }
    let at = line.find("\"hits\":[{").expect("hits") + "\"hits\":[".len();
    assert_eq!(
        raw_keys(&line, at),
        ["name", "kind", "visibility", "path", "line"]
    );
}

#[test]
fn a_method_named_by_its_module_and_name_resolves() {
    let world = World::new();
    for bin in BINS {
        let data = world.data(bin, &["callees", "tricky::greet"]);
        assert_eq!(
            data["symbol"]["name"], "crate::tricky::Guest::greet",
            "{data}"
        );
        assert_eq!(names(&data["hits"]), ["crate::tricky::other"], "{data}");
        assert_eq!(data["hits"][0]["display"], "src/tricky.rs::other", "{data}");
    }
}

#[test]
fn a_missed_name_suggests_the_symbols_with_its_last_segment() {
    let world = World::new();
    for bin in BINS {
        for command in ["callers", "callees", "impact", "show"] {
            let (code, env) = world.run(bin, &[command, "crate::util::deep::helper"]);
            assert_eq!(code, Some(3), "{command}: {env}");
            assert_eq!(env["error"]["kind"], "not_found", "{env}");
            assert_eq!(
                env["error"]["message"], "no symbol named crate::util::deep::helper",
                "{env}"
            );
            let suggested = names(&env["data"]["did_you_mean"]);
            assert!(suggested.contains(&"crate::util::helper"), "{env}");
            assert!(suggested.len() <= 10, "{env}");
            for candidate in env["data"]["did_you_mean"].as_array().expect("list") {
                let name = candidate["name"].as_str().expect("name");
                assert!(name.ends_with("helper"), "{env}");
                assert_eq!(candidate.get("stable_key").is_some(), command == "show");
            }
            let (_, none) = world.run(bin, &[command, "no_such_symbol"]);
            assert_eq!(none["data"], json!({ "did_you_mean": [] }), "{none}");
        }
    }
}

/// `data`'s new keys, recomputed from its listed hits: grouped by
/// `display`, the files of the named symbols, and the count.
fn regroup(hits: &[&Value], file_key: &str, line_key: &str) -> Value {
    let mut groups: Vec<(String, Vec<Value>, f64)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for hit in hits {
        let display = hit["display"].as_str().expect("display").to_owned();
        let confidence = hit["confidence"].as_f64().expect("confidence");
        files.push(hit[file_key].as_str().expect("file").to_owned());
        match groups.iter_mut().find(|(name, _, _)| *name == display) {
            Some((_, sites, low)) => {
                sites.push(hit[line_key].clone());
                *low = low.min(confidence);
            }
            None => groups.push((display, vec![hit[line_key].clone()], confidence)),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort();
    files.dedup();
    let distinct: Vec<Value> = groups
        .into_iter()
        .map(|(display, mut sites, low)| {
            sites.sort_by_key(Value::as_i64);
            json!({ "display": display, "sites": sites, "min_confidence": low })
        })
        .collect();
    json!({ "callers_distinct": distinct, "files": files, "sites": hits.len() })
}

fn new_keys(data: &Value) -> Value {
    json!({
        "callers_distinct": data["callers_distinct"],
        "files": data["files"],
        "sites": data["sites"],
    })
}

#[test]
fn callers_and_callees_group_their_hits() {
    let world = World::new();
    for bin in BINS {
        let callers = world.data(bin, &["callers", "crate::util::helper"]);
        let hits: Vec<&Value> = callers["hits"].as_array().expect("hits").iter().collect();
        assert!(hits.len() >= 2, "{callers}");
        assert_eq!(new_keys(&callers), regroup(&hits, "symbol_path", "line"));
        assert_eq!(
            callers["callers_distinct"][0]["display"],
            "src/lib.rs::entry"
        );
        let callees = world.data(bin, &["callees", "crate::entry"]);
        let hits: Vec<&Value> = callees["hits"].as_array().expect("hits").iter().collect();
        assert_eq!(new_keys(&callees), regroup(&hits, "symbol_path", "line"));
        assert!(
            hits.iter()
                .any(|hit| hit["display"] == "src/lib.rs::Counter::new"),
            "{callees}"
        );
    }
}

#[test]
fn the_new_keys_follow_the_old_ones() {
    let world = World::new();
    let (_, line) = world.line(BINS[0], &["callers", "crate::util::helper"]);
    let data = line.find("\"data\":").expect("data") + "\"data\":".len();
    assert_eq!(
        raw_keys(&line, data),
        [
            "symbol",
            "hits",
            "truncated",
            "callers_distinct",
            "files",
            "sites"
        ]
    );
    let hit = line.find("\"hits\":[").expect("hits") + "\"hits\":[".len();
    assert_eq!(
        raw_keys(&line, hit),
        [
            "name",
            "kind",
            "path",
            "line",
            "symbol_path",
            "confidence",
            "reason",
            "display"
        ]
    );
}

#[test]
fn impact_groups_its_hits_and_tests_and_summary_leaves_them_out() {
    let world = World::new();
    for bin in BINS {
        let data = world.data(bin, &["impact", "crate::util::deep::sink"]);
        let mut hits: Vec<&Value> = Vec::new();
        for group in data["groups"].as_array().expect("groups") {
            hits.extend(group["hits"].as_array().expect("hits"));
        }
        hits.extend(data["tests"].as_array().expect("tests"));
        assert!(!hits.is_empty(), "{data}");
        for hit in &hits {
            let display = hit["display"].as_str().expect("display");
            let name = hit["name"].as_str().expect("name");
            let path = hit["path"].as_str().expect("path");
            let last = name.rsplit("::").next().expect("segment");
            assert!(display.starts_with(&format!("{path}::")), "{hit}");
            assert!(display.ends_with(&format!("::{last}")), "{hit}");
        }
        assert_eq!(new_keys(&data), regroup(&hits, "path", "site_line"));
        let summary = world.data(bin, &["impact", "crate::util::deep::sink", "--summary"]);
        for key in ["callers_distinct", "files", "sites"] {
            assert!(summary.get(key).is_none(), "{summary}");
        }
    }
}

#[test]
fn a_budget_drops_the_new_keys_before_any_hit() {
    let world = World::new();
    let (_, full) = world.line(BINS[0], &["callers", "crate::util::helper"]);
    let full: Value = serde_json::from_str(&full).expect("envelope");
    let all = full["data"]["hits"].as_array().expect("hits").len();
    let (mut kept, mut lean) = (0, 0);
    for max in (40..=400).step_by(4) {
        let budget = max.to_string();
        let (code, env) = world.run(
            BINS[0],
            &["callers", "crate::util::helper", "--max-tokens", &budget],
        );
        if code != Some(0) {
            continue;
        }
        let data = &env["data"];
        let hits = data["hits"].as_array().expect("hits");
        if data.get("callers_distinct").is_some() {
            kept += 1;
            assert_eq!(hits.len(), all, "{env}");
            assert_eq!(data["tokens"]["omitted"], 0, "{env}");
        } else {
            lean += 1;
            assert!(
                data.get("files").is_none() && data.get("sites").is_none(),
                "{env}"
            );
            assert!(hits.iter().all(|hit| hit.get("display").is_none()), "{env}");
            let omitted = data["tokens"]["omitted"].as_u64().expect("omitted");
            assert_eq!(hits.len() as u64 + omitted, all as u64, "{env}");
        }
    }
    assert!(kept > 0 && lean > 0, "kept {kept}, lean {lean}");
}

/// `ceil(bytes / 3.5) + 1`, as the README's estimate defines it.
fn estimate(line: &str) -> u64 {
    (line.len() as u64 * 2).div_ceil(7) + 1
}

/// `full` with `tokens` spliced in at the end of `data`, nothing dropped,
/// and `used` settled on the estimate of that line.
fn whole(full: &str, max: u64) -> String {
    let at = full.find(",\"meta\":").expect("meta") - 1;
    let mut used = 0;
    loop {
        let line = format!(
            "{},\"tokens\":{{\"max\":{max},\"used\":{used},\"omitted\":0}}{}",
            &full[..at],
            &full[at..]
        );
        if estimate(&line) == used {
            return line;
        }
        used = estimate(&line);
    }
}

#[test]
fn the_new_keys_stay_exactly_when_the_whole_reply_fits() {
    let world = World::new();
    let (_, full) = world.line(BINS[0], &["callers", "crate::util::helper"]);
    // Whether the whole reply fits a budget is worked out here, without a
    // run: its cost changes only with the digits of the budget. The CLI is
    // then asked at each budget where that answer changes and the budgets
    // on either side, and at every budget that puts the whole reply at
    // exactly the room, where a `<` for `<=` would show.
    let cost = |max: u64| estimate(&whole(&full, max));
    let room = |max: u64| max * 95 / 100;
    let fits = |max: u64| cost(max) <= room(max);
    let exact: BTreeSet<u64> = (1..=400).filter(|&max| cost(max) == room(max)).collect();
    assert!(
        !exact.is_empty(),
        "no budget put the whole reply exactly at the room"
    );
    let mut budgets = exact.clone();
    for max in 2..=400_u64 {
        if fits(max) != fits(max - 1) {
            budgets.extend([max - 2, max - 1, max, max + 1]);
        }
    }
    assert!(budgets.iter().any(|&max| fits(max)) && budgets.iter().any(|&max| !fits(max)));
    for &max in &budgets {
        let budget = max.to_string();
        let (code, line) = world.line(
            BINS[0],
            &["callers", "crate::util::helper", "--max-tokens", &budget],
        );
        if fits(max) {
            assert_eq!(code, Some(0), "{max}: {line}");
            assert_eq!(line, whole(&full, max), "{max}");
        } else {
            assert!(!line.contains("callers_distinct"), "{max}: {line}");
        }
    }
    // A fixed sample that owes nothing to the model above: every 25th
    // budget of 40 to 400. Each reply is judged on what the CLI printed
    // alone. With the new keys, it is the unbudgeted reply whole, with
    // `tokens` added, nothing omitted and `used` within 95% of the
    // budget. Without them, no hit keeps its `display`. Across every
    // budget asked, the verdict only ever turns from dropped to kept, and
    // the model agrees with the CLI at each fixed budget.
    let stride: Vec<u64> = (40..=400).step_by(25).collect();
    let mut verdicts: Vec<(u64, bool)> = Vec::new();
    for &max in budgets.iter().chain(&stride) {
        let budget = max.to_string();
        let (code, line) = world.line(
            BINS[0],
            &["callers", "crate::util::helper", "--max-tokens", &budget],
        );
        let kept = observed(&full, max, code, &line);
        if stride.contains(&max) {
            assert_eq!(kept, fits(max), "the model and the CLI disagree at {max}");
        }
        verdicts.push((max, kept));
    }
    verdicts.sort_unstable();
    verdicts.dedup();
    assert!(
        verdicts.windows(2).all(|pair| pair[0].1 <= pair[1].1),
        "the new keys came and went: {verdicts:?}"
    );
    assert!(verdicts.iter().any(|v| v.1) && verdicts.iter().any(|v| !v.1));
}

/// Whether the reply at budget `max` kept the new keys, judged on the
/// printed line alone and checked either way, never through a model.
fn observed(full: &str, max: u64, code: Option<i32>, line: &str) -> bool {
    if code != Some(0) {
        assert!(!line.contains("callers_distinct"), "{max}: {line}");
        return false;
    }
    let envelope: Value = serde_json::from_str(line).expect("envelope");
    let data = &envelope["data"];
    let tokens = &data["tokens"];
    assert!(
        tokens["used"].as_u64().expect("used") <= max * 95 / 100,
        "{max}: {line}"
    );
    if data.get("callers_distinct").is_none() {
        let hits = data["hits"].as_array().expect("hits");
        assert!(
            hits.iter().all(|hit| hit.get("display").is_none()),
            "{max}: {line}"
        );
        return false;
    }
    assert_eq!(tokens["omitted"], 0, "{max}: {line}");
    let mut unbudgeted = envelope.clone();
    unbudgeted["data"]
        .as_object_mut()
        .expect("data")
        .remove("tokens");
    let full: Value = serde_json::from_str(full).expect("full");
    assert_eq!(
        unbudgeted, full,
        "{max}: kept the new keys but is not the whole reply"
    );
    true
}

/// A tree whose `target` has exactly `callers` callers, indexed.
fn fan_in(callers: usize) -> World {
    let world = World {
        repo: TempDir::new("answers-fan-repo"),
        data: TempDir::new("answers-fan-data"),
    };
    let mut lib = String::from("pub fn target() {}\n");
    for n in 0..callers {
        lib.push_str(&format!("\npub fn c{n:02}() {{\n    target();\n}}\n"));
    }
    let src = world.repo.path().join("src");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::write(
        world.repo.path().join("Cargo.toml"),
        "[package]\nname = \"fan\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("manifest");
    std::fs::write(src.join("lib.rs"), lib).expect("lib");
    let (code, line) = world.line(BINS[0], &["index", "."]);
    assert_eq!(code, Some(0), "{line}");
    world
}

#[test]
fn show_lists_20_callers_whole_and_flags_the_21st() {
    for (callers, truncated) in [(20, false), (21, true)] {
        let world = fan_in(callers);
        let data = world.data(BINS[0], &["show", "target"]);
        let side = &data["callers"];
        assert_eq!(side["count"], callers, "{data}");
        assert_eq!(side["hits"].as_array().expect("hits").len(), 20, "{data}");
        assert_eq!(side["truncated"], truncated, "{data}");
        assert!(side["hits"][0].get("display").is_none(), "{data}");
    }
}

#[test]
fn an_ambiguous_message_lists_every_candidate_with_no_tail_up_to_ten() {
    let world = World::new();
    for bin in BINS {
        let (code, env) = world.run(bin, &["callers", "helper"]);
        assert_eq!(code, Some(6), "{env}");
        let message = env["error"]["message"].as_str().expect("message");
        let total = env["data"]["total"].as_u64().expect("total");
        assert!(
            message.starts_with(&format!("{total} symbols match helper: ")),
            "{message}"
        );
        assert!(!message.contains("more"), "{message}");
        for candidate in env["data"]["candidates"].as_array().expect("candidates") {
            let name = candidate["name"].as_str().expect("name");
            let path = candidate["path"].as_str().expect("path");
            assert!(message.contains(&format!("{name} ({path}:")), "{message}");
        }
    }
}

/// The envelope line each `tools/call` returns, and its `isError`, from a
/// server rooted at `root`.
fn mcp(root: &Path, data: &Path, calls: &[(&str, Value)]) -> Vec<(bool, String)> {
    let mut input = vec![
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "answers", "version": "0" },
        }}),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    ];
    input.extend((1..).zip(calls).map(|(id, (name, arguments))| {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments } })
    }));
    let mut child = Command::new(BINS[0])
        .arg("mcp")
        .current_dir(root)
        .env("PHANTOM_FRACTAL_DATA", data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("serve");
    let mut stdin = child.stdin.take().expect("stdin");
    for message in &input {
        writeln!(stdin, "{message}").expect("write");
    }
    drop(stdin);
    let out = child.wait_with_output().expect("server");
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .skip(1)
        .map(|line| {
            let reply: Value = serde_json::from_str(line).expect("reply");
            let result = &reply["result"];
            let text = result["content"][0]["text"].as_str().expect("text");
            (result["isError"] == true, text.to_owned())
        })
        .collect()
}

/// A command line `retry`, run as printed. Its words here are all plain.
fn run_retry(world: &World, retry: &str) -> (Option<i32>, Value) {
    let words: Vec<&str> = retry.split(' ').collect();
    assert_eq!(words[0], "phantom-fractal", "{retry}");
    assert!(!retry.contains('\''), "{retry}");
    world.run(BINS[0], &words[1..])
}

/// The symbol's qualified name: `show` gives it as `qual_name`.
fn symbol_name(env: &Value) -> &Value {
    let symbol = &env["data"]["symbol"];
    symbol.get("qual_name").unwrap_or(&symbol["name"])
}

/// `fixtures/resolve` is the package `resolve-fixture`.
#[test]
fn a_workspace_crate_name_stands_for_crate_in_every_symbol_command() {
    let world = World::new();
    for command in ["show", "callers", "callees", "impact"] {
        let want = world.data(BINS[0], &[command, "crate::util::deep::sink"]);
        for name in [
            "resolve_fixture::util::deep::sink",
            "resolve-fixture::util::deep::sink",
        ] {
            assert_eq!(
                world.data(BINS[0], &[command, name]),
                want,
                "{command} {name}"
            );
        }
    }
    let path = world.data(
        BINS[0],
        &[
            "path",
            "resolve_fixture::util::helper",
            "resolve_fixture::util::deep::sink",
        ],
    );
    assert_eq!(path["found"], true, "{path}");
}

#[test]
fn a_path_and_line_name_the_definition_only_while_the_index_is_fresh() {
    let world = World::new();
    let at = "src/util.rs:5";
    for command in ["show", "callers", "callees", "impact"] {
        let (code, env) = world.run(BINS[0], &[command, at]);
        assert_eq!(code, Some(0), "{command}: {env}");
        assert_eq!(symbol_name(&env), "crate::util::helper", "{env}");
        assert_eq!(env["meta"]["stale"], false, "{env}");
    }
    let path = world.data(BINS[0], &["path", at, "crate::util::deep::sink"]);
    assert_eq!(path["found"], true, "{path}");

    let source = world.repo.path().join("src/util.rs");
    let text = std::fs::read_to_string(&source).expect("source");
    std::fs::write(&source, format!("\n{text}")).expect("edit");
    let stale = json!({
        "ok": false,
        "data": null,
        "error": {
            "kind": "usage",
            "message": "store: index is stale; run index before resolving path:line",
            "exit": 1,
        },
        "schema": 1,
    });
    for args in [
        &["show", at][..],
        &["callers", at],
        &["callees", at],
        &["impact", at],
        &["path", at, "crate::util::deep::sink"],
        &["path", "crate::util::helper", at],
    ] {
        let (code, env) = world.run(BINS[0], args);
        assert_eq!((code, &env), (Some(1), &stale), "{args:?}");
    }
    let over_mcp = mcp(
        world.repo.path(),
        world.data.path(),
        &[("callers", json!({ "symbol": at }))],
    );
    let envelope: Value = serde_json::from_str(&over_mcp[0].1).expect("envelope");
    assert_eq!((over_mcp[0].0, &envelope), (true, &stale));
    // A name still answers from the stale index, and says it is stale.
    let (code, env) = world.run(BINS[0], &["callers", "crate::util::helper"]);
    assert_eq!(
        (code, &env["meta"]["stale"]),
        (Some(0), &json!(true)),
        "{env}"
    );

    let (code, line) = world.line(BINS[0], &["index", "."]);
    assert_eq!(code, Some(0), "{line}");
    let data = world.data(BINS[0], &["callers", "src/util.rs:6"]);
    assert_eq!(data["symbol"]["name"], "crate::util::helper", "{data}");
}

/// Only one symbol is named `sink`, and a miss still answers for none.
#[test]
fn a_miss_is_never_answered_for_the_one_suggestion() {
    let world = World::new();
    for command in ["show", "callers", "callees", "impact"] {
        let (code, env) = world.run(BINS[0], &[command, "nowhere::sink"]);
        assert_eq!((code, &env["ok"]), (Some(3), &json!(false)), "{env}");
        assert_eq!(env["error"]["kind"], "not_found", "{env}");
        assert!(env.get("meta").is_none(), "{env}");
        let data = env["data"].as_object().expect("data");
        let keys: Vec<&str> = data.keys().map(String::as_str).collect();
        assert_eq!(keys, ["did_you_mean", "retry"], "{env}");
        assert_eq!(
            names(&env["data"]["did_you_mean"]),
            ["crate::util::deep::sink"]
        );
        let retry = env["data"]["retry"].as_str().expect("retry");
        assert_eq!(
            retry,
            format!("phantom-fractal {command} -- src/util/deep.rs::sink")
        );
        let (code, again) = run_retry(&world, retry);
        assert_eq!(code, Some(0), "{retry}: {again}");
        assert_eq!(symbol_name(&again), "crate::util::deep::sink");
    }
}

#[test]
fn a_missed_end_of_a_path_is_retried_in_its_own_place() {
    let world = World::new();
    let good = "crate::util::deep::sink";
    let from = "crate::util::helper";
    for (args, retry) in [
        (
            ["path", "nowhere::sink", good],
            format!("phantom-fractal path -- src/util/deep.rs::sink {good}"),
        ),
        (
            ["path", from, "nowhere::sink"],
            format!("phantom-fractal path -- {from} src/util/deep.rs::sink"),
        ),
    ] {
        let (code, env) = world.run(BINS[0], &args);
        assert_eq!(code, Some(3), "{env}");
        assert_eq!(env["data"]["retry"], retry, "{env}");
        let (code, again) = run_retry(&world, &retry);
        assert_eq!(code, Some(0), "{retry}: {again}");
    }
}

#[test]
fn a_line_that_starts_nothing_suggests_the_nearest_first() {
    let world = World::new();
    let (code, env) = world.run(BINS[0], &["callers", "src/util.rs:6"]);
    assert_eq!((code, &env["ok"]), (Some(3), &json!(false)), "{env}");
    let suggested = names(&env["data"]["did_you_mean"]);
    assert_eq!(suggested[0], "crate::util::helper", "{env}");
    let retry = env["data"]["retry"].as_str().expect("retry");
    assert_eq!(retry, "phantom-fractal callers -- src/util.rs::helper");
    let (code, again) = run_retry(&world, retry);
    assert_eq!(code, Some(0), "{again}");
    // A file that is not indexed has nothing to suggest.
    let (code, env) = world.run(BINS[0], &["callers", "src/nope.rs:6"]);
    assert_eq!(
        (code, &env["data"]),
        (Some(3), &json!({ "did_you_mean": [] }))
    );
}

#[test]
fn a_miss_reads_the_same_over_mcp() {
    let world = World::new();
    let (_, line) = world.line(BINS[0], &["callers", "nowhere::sink"]);
    let over_mcp = mcp(
        world.repo.path(),
        world.data.path(),
        &[("callers", json!({ "symbol": "nowhere::sink" }))],
    );
    assert_eq!(over_mcp, [(true, line)]);
}

/// `args` with `--limit limit`: the envelope.
fn limited(world: &World, args: &[&str], limit: &str) -> Value {
    let mut with = args.to_vec();
    with.extend(["--limit", limit]);
    let (code, env) = world.run(BINS[0], &with);
    assert_eq!(code, Some(0), "{with:?}: {env}");
    env
}

#[test]
fn a_limit_above_100_is_cut_to_100_and_the_reply_says_so() {
    let world = World::new();
    let noted = json!({ "asked": 200, "used": 100 });
    let sink = ["impact", "crate::util::deep::sink"];
    for args in [&sink[..], &["rank"], &["rank", "--files"]] {
        let over = limited(&world, args, "200");
        assert_eq!(over["meta"]["limit"], noted, "{over}");
        assert_eq!(over["data"]["truncated"], true, "{over}");
        let cap = limited(&world, args, "100");
        assert!(cap["meta"].get("limit").is_none(), "{cap}");
        // Apart from what says so, the reply is the one 100 gives.
        let mut same = over["data"].clone();
        same["truncated"] = cap["data"]["truncated"].clone();
        if cap["data"].get("truncated").is_none() {
            same.as_object_mut().expect("data").remove("truncated");
        }
        assert_eq!(same, cap["data"], "{args:?}");
    }
    let cap = limited(&world, &sink, "100");
    assert!(cap["data"].get("truncated").is_none(), "{cap}");
    assert!(world.data(BINS[0], &sink).get("truncated").is_none());
    assert_eq!(
        limited(&world, &["rank"], "100")["data"]["truncated"],
        false
    );

    // A budget that drops entries keeps what says the limit was cut.
    let fitted = limited(
        &world,
        &["impact", "crate::util::deep::sink", "--max-tokens", "250"],
        "200",
    );
    assert_eq!(fitted["meta"]["limit"], noted, "{fitted}");
    assert_eq!(fitted["data"]["truncated"], true, "{fitted}");
    assert!(
        fitted["data"]["tokens"]["used"].as_u64() <= Some(250),
        "{fitted}"
    );

    for limit in ["0", "-1", "many", "2.5"] {
        for args in [&sink[..], &["rank"]] {
            let mut with = args.to_vec();
            with.extend(["--limit", limit]);
            let (code, line) = world.line(BINS[0], &with);
            assert_eq!((code, line.as_str()), (Some(1), ""), "{with:?}");
        }
    }
}

#[test]
fn a_cochange_limit_above_100_is_cut_in_both_modes_and_over_mcp() {
    let repo = Repo::new("answers-limit", &history());
    let data = TempDir::new("answers-limit-data");
    let line = |args: &[&str]| {
        let out = Command::new(BINS[0])
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", data.path())
            .current_dir(&repo.root)
            .output()
            .expect("run");
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout)
            .expect("utf-8")
            .trim_end()
            .to_owned()
    };
    line(&["index", "."]);
    let mut printed = Vec::new();
    for args in [
        &["cochange", "src/a.rs"][..],
        &["cochange", "src/a.rs", "--count"],
        &["impact", "a::fa"],
        &["rank"],
    ] {
        let mut with = args.to_vec();
        with.extend(["--limit", "200"]);
        let text = line(&with);
        let env: Value = serde_json::from_str(&text).expect("envelope");
        assert_eq!(
            env["meta"]["limit"],
            json!({ "asked": 200, "used": 100 }),
            "{text}"
        );
        assert_eq!(env["data"]["truncated"], true, "{text}");
        printed.push((false, text));
    }
    let over_mcp = mcp(
        &repo.root,
        data.path(),
        &[
            ("cochange", json!({ "file": "src/a.rs", "limit": 200 })),
            (
                "cochange",
                json!({ "file": "src/a.rs", "limit": 200, "mode": "count" }),
            ),
            ("impact", json!({ "symbol": "a::fa", "limit": 200 })),
            ("top_ranked", json!({ "limit": 200 })),
        ],
    );
    assert_eq!(over_mcp, printed);
}
