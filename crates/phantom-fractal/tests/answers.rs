//! Answers agents can use (SPEC-w14 G1b): `find` over methods, a missed
//! lookup's `did_you_mean`, and the `display`, `callers_distinct`,
//! `files` and `sites` keys of `callers`, `callees` and `impact`, which
//! sit after the old keys and go first under a budget.

use std::process::Command;

use serde_json::{Value, json};

mod support;

use support::{TempDir, copy_fixture, raw_keys};

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
