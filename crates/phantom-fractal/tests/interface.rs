//! The SPEC-w15 G1b surface keys: `impact --limit`, `display` on the
//! candidates of an ambiguous lookup, and `reexported` with
//! `confirm_with_grep` on `callers` and `impact`.
//!
//! `fixtures/impact` and a small crate written here are copied to temp
//! directories and indexed into a temp data directory.

mod support;

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use support::{TempDir, copy_fixture, raw_keys, value_at};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

/// A crate with two `twin`s and two `Pair::new`s in sibling modules, a
/// `helper` re-exported by `pub use`, a `hidden` imported by a private
/// `use`, and a `lonely` nothing imports.
const PROBE: [(&str, &str); 5] = [
    (
        "Cargo.toml",
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    (
        "src/lib.rs",
        "mod inner;\nmod other;\nmod quiet;\npub use inner::helper;\nuse quiet::hidden;\n\n\
         pub fn caller() {\n    helper();\n}\n\npub fn lonely() {}\n\npub fn uses_lonely() {\n    \
         lonely();\n}\n\npub fn uses_hidden() {\n    hidden();\n}\n",
    ),
    ("src/quiet.rs", "pub fn hidden() {}\n"),
    (
        "src/inner.rs",
        "pub fn helper() {}\n\npub fn twin() {}\n\npub struct Pair;\n\nimpl Pair {\n    \
         pub fn new() -> Self {\n        Pair\n    }\n}\n",
    ),
    (
        "src/other.rs",
        "pub fn twin() {}\n\npub struct Pair;\n\nimpl Pair {\n    pub fn new() -> Self {\n        \
         Pair\n    }\n}\n",
    ),
];

/// An indexed tree and its data directory.
struct World {
    tree: TempDir,
    data: TempDir,
}

impl World {
    fn impact() -> Self {
        let world = Self::empty("impact");
        copy_fixture("impact", world.tree.path());
        world.index();
        world
    }

    fn probe() -> Self {
        let world = Self::empty("probe");
        for (path, text) in PROBE {
            let path = world.tree.path().join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, text).expect("write");
        }
        world.index();
        world
    }

    fn empty(label: &str) -> Self {
        Self {
            tree: TempDir::new(&format!("w15-{label}-tree")),
            data: TempDir::new(&format!("w15-{label}-data")),
        }
    }

    fn index(&self) {
        let (code, line) = self.run(BINS[0], &["index", "."]);
        assert_eq!(code, Some(0), "{line}");
    }

    /// The exit code and stdout, newline removed, of `bin args` run in the
    /// tree.
    fn run(&self, bin: &str, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(bin)
            .args(args)
            .current_dir(self.tree.path())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run");
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        (
            output.status.code(),
            stdout.trim_end_matches('\n').to_owned(),
        )
    }

    /// The envelope of a run that must print one.
    fn envelope(&self, bin: &str, args: &[&str]) -> (Option<i32>, Value, String) {
        let (code, line) = self.run(bin, args);
        let value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("{args:?}: {line}"));
        (code, value, line)
    }
}

/// The printed keys of `data` in `line`.
fn data_keys(line: &str) -> Vec<String> {
    raw_keys(line, value_at(line, "data", 0))
}

fn hits_of(group: &Value) -> &Vec<Value> {
    group["hits"].as_array().expect("hits")
}

#[test]
fn impact_without_limit_has_no_limit_key() {
    let world = World::impact();
    for bin in BINS {
        let (code, env, line) = world.envelope(bin, &["impact", "root_fn"]);
        assert_eq!(code, Some(0), "{line}");
        assert!(env["data"].get("limit").is_none(), "{line}");
    }
}

#[test]
fn impact_limit_keeps_the_shallowest_hits_and_counts_the_rest() {
    let world = World::impact();
    for bin in BINS {
        let (_, whole, _) = world.envelope(bin, &["impact", "root_fn"]);
        let all: Vec<&Value> = whole["data"]["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .flat_map(hits_of)
            .collect();
        let tests = whole["data"]["tests"].as_array().expect("tests").len();
        assert!(all.len() > 3 && tests >= 2, "{whole}");
        for limit in [1usize, 3, all.len(), 100] {
            let text = limit.to_string();
            let (code, env, line) = world.envelope(bin, &["impact", "root_fn", "--limit", &text]);
            assert_eq!(code, Some(0), "{line}");
            let data = &env["data"];
            let groups = data["groups"].as_array().expect("groups");
            let kept: Vec<&Value> = groups.iter().flat_map(hits_of).collect();
            assert_eq!(kept.len(), limit.min(all.len()), "{line}");
            assert_eq!(
                kept.as_slice(),
                &all[..kept.len()],
                "the first hits, in order: {line}"
            );
            for (group, before) in groups
                .iter()
                .zip(whole["data"]["groups"].as_array().expect("g"))
            {
                assert_eq!(group["depth"], before["depth"], "{line}");
                assert_eq!(group["total"], before["total"], "{line}");
                let cut = hits_of(group).len() < hits_of(before).len();
                assert_eq!(
                    group["truncated"],
                    cut || before["truncated"] == true,
                    "{line}"
                );
            }
            let listed_tests = data["tests"].as_array().expect("tests").len();
            assert_eq!(listed_tests, limit.min(tests), "{line}");
            assert_eq!(data["limit"]["max"], limit, "{line}");
            assert_eq!(
                data["limit"]["omitted_hits"],
                all.len() - kept.len(),
                "{line}"
            );
            assert_eq!(
                data["limit"]["omitted_tests"],
                tests - listed_tests,
                "{line}"
            );
            assert_eq!(data["sites"], kept.len() + listed_tests, "{line}");
            let keys = data_keys(&line);
            let at = keys.iter().position(|key| key == "limit").expect("limit");
            assert_eq!(
                keys[at - 1],
                "sites",
                "limit follows the W14 keys: {keys:?}"
            );
        }
    }
}

#[test]
fn impact_limit_under_summary_and_no_tests() {
    let world = World::impact();
    let (_, env, line) =
        world.envelope(BINS[0], &["impact", "root_fn", "--limit", "2", "--summary"]);
    assert!(env["data"].get("groups").is_none(), "{line}");
    assert_eq!(
        env["data"]["limit"],
        serde_json::json!({ "max": 2, "omitted_hits": 0, "omitted_tests": 0 }),
        "{line}"
    );
    let (_, env, line) = world.envelope(
        BINS[0],
        &["impact", "root_fn", "--limit", "1", "--no-tests"],
    );
    assert!(env["data"].get("tests").is_none(), "{line}");
    assert_eq!(env["data"]["limit"]["omitted_tests"], 0, "{line}");
}

#[test]
fn impact_limit_out_of_range_or_with_diff_is_usage() {
    let world = World::impact();
    for bin in BINS {
        for args in [
            &["impact", "root_fn", "--limit", "0"][..],
            &["impact", "root_fn", "--limit", "101"],
            &["impact", "root_fn", "--limit", "x"],
            &["impact", "--diff", "--limit", "3"],
        ] {
            let (code, _) = world.run(bin, args);
            assert_eq!(code, Some(1), "{args:?}");
        }
    }
}

#[test]
fn impact_limit_goes_first_under_a_budget() {
    let world = World::impact();
    let fitted = smallest_fit(&world, &["impact", "root_fn", "--limit", "3"]);
    assert!(!fitted.contains("\"limit\""), "{fitted}");
}

/// The reply at the smallest budget, in steps of 10, that fits.
fn smallest_fit(world: &World, args: &[&str]) -> String {
    for max in (10..=4000).step_by(10) {
        let text = max.to_string();
        let mut with = args.to_vec();
        with.extend(["--max-tokens", &text]);
        let (code, line) = world.run(BINS[0], &with);
        if code == Some(0) {
            return line;
        }
    }
    panic!("nothing fits: {args:?}");
}

#[test]
fn ambiguous_candidates_carry_their_display() {
    let world = World::probe();
    let cases: [(&str, [&str; 2]); 2] = [
        ("twin", ["src/inner.rs::twin", "src/other.rs::twin"]),
        (
            "Pair::new",
            ["src/inner.rs::Pair::new", "src/other.rs::Pair::new"],
        ),
    ];
    for bin in BINS {
        for command in ["callers", "callees", "impact", "show"] {
            for (query, displays) in cases {
                let (code, env, line) = world.envelope(bin, &[command, query]);
                assert_eq!(code, Some(6), "{line}");
                assert_eq!(env["error"]["kind"], "ambiguous", "{line}");
                let candidates = env["data"]["candidates"].as_array().expect("candidates");
                let got: Vec<&str> = candidates
                    .iter()
                    .map(|candidate| candidate["display"].as_str().expect("display"))
                    .collect();
                assert_eq!(got, displays, "{line}");
                let first = value_at(&line, "candidates", 0) + 1;
                let keys = raw_keys(&line, first);
                assert_eq!(keys.last().map(String::as_str), Some("display"), "{keys:?}");
                let old: &[&str] = if command == "show" {
                    &["name", "kind", "path", "line", "stable_key"]
                } else {
                    &["name", "kind", "path", "line"]
                };
                assert_eq!(keys[..keys.len() - 1], *old, "{line}");
            }
        }
    }
}

#[test]
fn did_you_mean_carries_no_display() {
    let world = World::probe();
    let (code, env, line) = world.envelope(BINS[0], &["callers", "nowhere::twin"]);
    assert_eq!(code, Some(3), "{line}");
    let suggestions = env["data"]["did_you_mean"]
        .as_array()
        .expect("did_you_mean");
    assert!(!suggestions.is_empty(), "{line}");
    assert!(
        suggestions.iter().all(|s| s.get("display").is_none()),
        "{line}"
    );
}

#[test]
fn a_reexported_target_is_flagged_on_callers_and_impact() {
    let world = World::probe();
    for bin in BINS {
        for command in ["callers", "impact"] {
            let (code, env, line) = world.envelope(bin, &[command, "helper"]);
            assert_eq!(code, Some(0), "{line}");
            assert_eq!(env["data"]["reexported"], true, "{line}");
            assert_eq!(env["data"]["confirm_with_grep"], true, "{line}");
            let keys = data_keys(&line);
            assert_eq!(
                keys[keys.len() - 3..],
                ["sites", "reexported", "confirm_with_grep"],
                "{line}"
            );
        }
        let (_, env, line) = world.envelope(bin, &["callees", "helper"]);
        assert!(env["data"].get("reexported").is_none(), "{line}");
    }
}

#[test]
fn a_target_nothing_reexports_has_no_flag() {
    let world = World::probe();
    for bin in BINS {
        for command in ["callers", "impact"] {
            for name in ["lonely", "hidden"] {
                let (code, env, line) = world.envelope(bin, &[command, name]);
                assert_eq!(code, Some(0), "{line}");
                assert!(env["data"].get("reexported").is_none(), "{line}");
                assert!(env["data"].get("confirm_with_grep").is_none(), "{line}");
            }
        }
    }
}

/// The `display` of each ambiguous candidate names that one symbol when
/// passed back, through the store's display-spelling fallback.
#[test]
fn a_candidate_display_passed_back_picks_that_symbol() {
    let world = World::probe();
    for bin in BINS {
        for query in ["twin", "Pair::new"] {
            let (_, env, line) = world.envelope(bin, &["callers", query]);
            let candidates = env["data"]["candidates"].as_array().expect("candidates");
            assert_eq!(candidates.len(), 2, "{line}");
            for candidate in candidates {
                let display = candidate["display"].as_str().expect("display");
                for command in ["callers", "impact", "show"] {
                    let (code, picked, line) = world.envelope(bin, &[command, display]);
                    assert_eq!(code, Some(0), "{command} {display}: {line}");
                    let symbol = &picked["data"]["symbol"];
                    assert_eq!(symbol["path"], candidate["path"], "{line}");
                    assert_eq!(symbol["line"], candidate["line"], "{line}");
                }
            }
        }
    }
}

#[test]
fn the_flag_goes_first_under_a_budget() {
    let world = World::probe();
    for command in ["callers", "impact"] {
        let fitted = smallest_fit(&world, &[command, "helper"]);
        assert!(!fitted.contains("reexported"), "{fitted}");
        assert!(!fitted.contains("confirm_with_grep"), "{fitted}");
    }
}

/// The tree is not written by any of the reads above.
#[test]
fn reads_leave_the_tree_alone() {
    let world = World::probe();
    let before = listing(world.tree.path());
    for args in [
        &["callers", "helper"][..],
        &["impact", "helper", "--limit", "1"],
        &["show", "twin"],
    ] {
        let _ = world.run(BINS[0], args);
    }
    assert_eq!(listing(world.tree.path()), before);
}

fn listing(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path
                    .strip_prefix(root)
                    .expect("under root")
                    .display()
                    .to_string();
                out.push((name, fs::read(&path).expect("read")));
            }
        }
    }
    out.sort();
    out
}
