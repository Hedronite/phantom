//! `impact` against a temp copy of `fixtures/impact`, whose targets its
//! `EXPECTED.md` lists, with the data directory in a second temp directory.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::atomic::{AtomicU32, Ordering};

use phantom_fractal_store::Store;
use serde_json::Value;

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/impact");

const EXPECTED: &str = include_str!("../../../fixtures/impact/EXPECTED.md");

/// The `data` keys of a full answer, of `--no-tests`, and of `--summary`.
const KEYS: [&str; 8] = [
    "counts", "exact", "groups", "options", "reasons", "risk", "symbol", "tests",
];
const NO_TESTS_KEYS: [&str; 7] = [
    "counts", "exact", "groups", "options", "reasons", "risk", "symbol",
];
const SUMMARY_KEYS: [&str; 6] = ["counts", "exact", "options", "reasons", "risk", "symbol"];

const HIT_KEYS: [&str; 8] = [
    "confidence",
    "kind",
    "line",
    "name",
    "path",
    "reason",
    "site_line",
    "site_path",
];

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-impact-{label}-{}-{}",
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
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&dest).expect("create dir");
            copy_tree(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).expect("copy file");
        }
    }
}

/// The one `graph.db` under `dir`.
fn find_database(dir: &Path) -> PathBuf {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "graph.db") {
                found.push(path);
            }
        }
    }
    assert_eq!(found.len(), 1, "expected one graph.db: {found:?}");
    found.remove(0)
}

/// Every string in `value`, at any depth.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        Value::Object(map) => map.values().flat_map(strings).collect(),
        _ => Vec::new(),
    }
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect()
}

struct Fixture {
    repo: TempDir,
    data: TempDir,
}

impl Fixture {
    /// A copy of `fixtures/impact`, indexed.
    fn indexed() -> Self {
        let fixture = Self {
            repo: TempDir::new("repo"),
            data: TempDir::new("data"),
        };
        copy_tree(Path::new(FIXTURE), fixture.repo.path());
        let repo = fixture.repo.path().to_str().expect("utf-8 temp path");
        let (code, env) = fixture.run(BINS[1], &["index", repo]);
        assert_eq!(code, Some(0), "index failed: {env}");
        assert_eq!(env["data"]["skipped"], 0, "{env}");
        fixture
    }

    /// Runs `bin` in the repo and checks that nothing it printed names
    /// either temp directory or any absolute path.
    fn run(&self, bin: &str, args: &[&str]) -> (Option<i32>, Value) {
        let out = Command::new(bin)
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .current_dir(self.repo.path())
            .output()
            .unwrap_or_else(|err| panic!("cannot run {bin}: {err}"));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for dir in [self.repo.path(), self.data.path()] {
            let dir = dir.to_str().expect("utf-8");
            assert!(!text.contains(dir), "{bin} {args:?} printed {dir}: {text}");
        }
        let env: Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|err| panic!("stdout is not one JSON envelope ({err}): {text}"));
        for value in strings(&env) {
            assert!(
                !Path::new(value).is_absolute(),
                "{bin} {args:?} printed an absolute path {value:?}: {text}"
            );
        }
        assert_eq!(env["schema"], 1, "{bin} {args:?}: {env}");
        (out.status.code(), env)
    }

    /// `impact ARGS` that must succeed; its `data`.
    fn impact(&self, bin: &str, args: &[&str]) -> Value {
        let args: Vec<&str> = ["impact"].iter().chain(args).copied().collect();
        let (code, env) = self.run(bin, &args);
        assert_eq!(code, Some(0), "{bin} {args:?}: {env}");
        assert_eq!(env["ok"], true, "{bin} {args:?}: {env}");
        assert_eq!(env["error"], Value::Null, "{bin} {args:?}: {env}");
        env["data"].clone()
    }
}

/// One `## Target NAME` section of `EXPECTED.md`: its name and its
/// `- key: value` lines.
struct Section {
    name: String,
    lines: Vec<(String, String)>,
}

fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            // Any other heading ends the current target.
            let name = heading.strip_prefix("Target ").unwrap_or("").trim();
            out.push(Section {
                name: name.to_owned(),
                lines: Vec::new(),
            });
            continue;
        }
        let (Some(section), Some(entry)) = (out.last_mut(), line.strip_prefix("- ")) else {
            continue;
        };
        if !section.name.is_empty()
            && let Some((key, value)) = entry.split_once(": ")
        {
            section.lines.push((key.to_owned(), value.to_owned()));
        }
    }
    out.retain(|section| !section.name.is_empty());
    out
}

/// `DEPTH PATH:LINE NAME CONFIDENCE REASON SITE_PATH:SITE_LINE`, with the
/// last segment of the qualified name, as `EXPECTED.md` writes it.
fn hit_line(depth: &Value, hit: &Value) -> String {
    let name = hit["name"].as_str().expect("name");
    let bare = name.rsplit("::").next().unwrap_or(name);
    format!(
        "{depth} {}:{} {bare} {} {} {}:{}",
        hit["path"].as_str().expect("path"),
        hit["line"],
        hit["confidence"],
        hit["reason"].as_str().expect("reason"),
        hit["site_path"].as_str().expect("site_path"),
        hit["site_line"],
    )
}

/// The `EXPECTED.md` lines a CLI answer gives.
fn rendered(data: &Value) -> Vec<(String, String)> {
    let reasons: Vec<String> = data["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|r| {
            let name = r["reason"].as_str().expect("reason");
            match r["count"].as_u64() {
                Some(count) => format!("{name}={count}"),
                None => name.to_owned(),
            }
        })
        .collect();
    let reasons = if reasons.is_empty() {
        "none".to_owned()
    } else {
        reasons.join(", ")
    };
    let c = &data["counts"];
    let mut out = vec![
        (
            "risk".to_owned(),
            data["risk"].as_str().expect("risk").to_owned(),
        ),
        ("exact".to_owned(), data["exact"].to_string()),
        ("reasons".to_owned(), reasons),
        (
            "counts".to_owned(),
            format!(
                "direct {}, total {}, packages {}, tests {}, below_floor {}",
                c["direct"], c["total"], c["packages"], c["tests"], c["below_floor"]
            ),
        ),
    ];
    for group in data["groups"].as_array().expect("groups") {
        let hits = group["hits"].as_array().expect("hits");
        assert_eq!(group["total"], hits.len(), "{group}");
        assert_eq!(group["truncated"], false, "{group}");
        out.extend(
            hits.iter()
                .map(|h| ("hit".to_owned(), hit_line(&group["depth"], h))),
        );
    }
    for test in data["tests"].as_array().expect("tests") {
        out.push(("test".to_owned(), hit_line(&test["depth"], test)));
    }
    out
}

/// Equal, with the confidence field of a hit line compared to 1e-9.
fn same(want: &(String, String), got: &(String, String)) -> bool {
    if want.0 != got.0 {
        return false;
    }
    if want.0 != "hit" && want.0 != "test" {
        return want.1 == got.1;
    }
    let want: Vec<&str> = want.1.split(' ').collect();
    let got: Vec<&str> = got.1.split(' ').collect();
    want.len() == 6
        && got.len() == 6
        && want.iter().zip(&got).enumerate().all(|(i, (w, g))| {
            if i != 3 {
                return w == g;
            }
            match (w.parse::<f64>(), g.parse::<f64>()) {
                (Ok(w), Ok(g)) => (w - g).abs() < 1e-9,
                _ => false,
            }
        })
}

#[test]
fn impact_matches_every_expected_target() {
    let fixture = Fixture::indexed();
    let targets = sections(EXPECTED);
    let names: Vec<&str> = targets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "root_fn",
            "hidden",
            "used_in_macros",
            "ping",
            "area",
            "doubled",
            "report"
        ]
    );
    for bin in BINS {
        for section in &targets {
            let data = fixture.impact(bin, &[section.name.as_str()]);
            let got = rendered(&data);
            let matches = got.len() == section.lines.len()
                && section.lines.iter().zip(&got).all(|(w, g)| same(w, g));
            assert!(
                matches,
                "{bin} {}\nwant {:#?}\ngot {:#?}",
                section.name, section.lines, got
            );
            let symbol = &data["symbol"];
            let name = symbol["name"].as_str().expect("symbol name");
            assert!(name.ends_with(&format!("::{}", section.name)), "{symbol}");
        }
    }
}

#[test]
fn impact_data_has_the_spec_keys() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let data = fixture.impact(bin, &["root_fn"]);
        assert_eq!(keys(&data), BTreeSet::from(KEYS), "{bin}: {data}");
        assert_eq!(
            keys(&data["symbol"]),
            BTreeSet::from(["kind", "line", "name", "path"]),
            "{bin}: {data}"
        );
        assert_eq!(
            keys(&data["counts"]),
            BTreeSet::from(["below_floor", "direct", "packages", "tests", "total"]),
            "{bin}: {data}"
        );
        assert_eq!(
            keys(&data["options"]),
            BTreeSet::from(["depth", "include_tests", "min_confidence"]),
            "{bin}: {data}"
        );
        assert_eq!(data["options"]["depth"], 3, "{bin}: {data}");
        assert_eq!(data["options"]["min_confidence"], 0.2, "{bin}: {data}");
        assert_eq!(data["options"]["include_tests"], true, "{bin}: {data}");
        for reason in data["reasons"].as_array().expect("reasons") {
            assert_eq!(
                keys(reason),
                BTreeSet::from(["count", "reason"]),
                "{bin}: {data}"
            );
        }
        for group in data["groups"].as_array().expect("groups") {
            assert_eq!(
                keys(group),
                BTreeSet::from(["depth", "hits", "total", "truncated"]),
                "{bin}: {data}"
            );
            for hit in group["hits"].as_array().expect("hits") {
                assert_eq!(keys(hit), BTreeSet::from(HIT_KEYS), "{bin}: {hit}");
            }
        }
        let tests = data["tests"].as_array().expect("tests");
        assert_eq!(tests.len(), 2, "{bin}: {data}");
        let mut test_keys = BTreeSet::from(HIT_KEYS);
        test_keys.insert("depth");
        for test in tests {
            assert_eq!(keys(test), test_keys, "{bin}: {test}");
        }
    }
}

#[test]
fn summary_drops_only_groups_and_tests() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let full = fixture.impact(bin, &["root_fn"]);
        for args in [["root_fn", "--summary"], ["--summary", "root_fn"]] {
            let data = fixture.impact(bin, &args);
            assert_eq!(keys(&data), BTreeSet::from(SUMMARY_KEYS), "{bin}: {data}");
            for key in SUMMARY_KEYS {
                assert_eq!(data[key], full[key], "{bin} {args:?}: {key}");
            }
        }
        let data = fixture.impact(bin, &["root_fn", "--summary", "--no-tests"]);
        assert_eq!(keys(&data), BTreeSet::from(SUMMARY_KEYS), "{bin}: {data}");
        assert_eq!(data["counts"]["tests"], 0, "{bin}: {data}");
    }
}

#[test]
fn no_tests_leaves_the_tests_out() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let full = fixture.impact(bin, &["root_fn"]);
        let data = fixture.impact(bin, &["root_fn", "--no-tests"]);
        assert_eq!(keys(&data), BTreeSet::from(NO_TESTS_KEYS), "{bin}: {data}");
        assert_eq!(data["options"]["include_tests"], false, "{bin}: {data}");
        assert_eq!(data["counts"]["tests"], 0, "{bin}: {data}");
        assert_eq!(data["groups"], full["groups"], "{bin}: {data}");
        assert_eq!(data["counts"]["total"], full["counts"]["total"], "{bin}");
    }
}

#[test]
fn depth_and_min_confidence_bound_the_walk() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let data = fixture.impact(bin, &["root_fn", "--depth", "1"]);
        let depths: Vec<&Value> = data["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .map(|group| &group["depth"])
            .collect();
        assert_eq!(depths, [&Value::from(1)], "{bin}: {data}");
        assert_eq!(data["options"]["depth"], 1, "{bin}: {data}");
        assert_eq!(data["counts"]["total"], 5, "{bin}: {data}");

        // `four` is depth 4 below `root_fn`; the default depth leaves it out.
        let data = fixture.impact(bin, &["root_fn", "--depth", "5"]);
        let deepest = data["groups"].as_array().expect("groups").last().cloned();
        assert_eq!(deepest.expect("a group")["depth"], 4, "{bin}: {data}");

        // `area` reaches `doubled` at 0.9, `run` at 0.855 and `main` at
        // 0.81225: a floor of 0.86 keeps `doubled` and stops `run`.
        let data = fixture.impact(bin, &["area", "--min-confidence", "0.86"]);
        assert_eq!(data["counts"]["total"], 1, "{bin}: {data}");
        assert_eq!(data["counts"]["below_floor"], 1, "{bin}: {data}");
        assert_eq!(data["options"]["min_confidence"], 0.86, "{bin}: {data}");

        let data = fixture.impact(bin, &["area", "--min-confidence", "1"]);
        assert_eq!(data["counts"]["total"], 0, "{bin}: {data}");
        assert_eq!(data["groups"], Value::Array(Vec::new()), "{bin}: {data}");
    }
}

/// A bad option is usage, exit 1, before any lookup. Clap reports it on
/// stderr and prints no envelope.
#[test]
fn bad_options_are_usage() {
    let fixture = Fixture::indexed();
    let cases: [&[&str]; 12] = [
        &["root_fn", "--depth", "0"],
        &["root_fn", "--depth", "6"],
        &["root_fn", "--depth", "-1"],
        &["root_fn", "--depth", "two"],
        &["root_fn", "--min-confidence", "0"],
        &["root_fn", "--min-confidence", "-0.1"],
        &["root_fn", "--min-confidence", "1.5"],
        &["root_fn", "--min-confidence", "NaN"],
        &["root_fn", "--min-confidence", "inf"],
        &["root_fn", "--min-confidence", "high"],
        &["root_fn", "extra"],
        &[],
    ];
    for bin in BINS {
        for args in cases {
            let out = Command::new(bin)
                .arg("impact")
                .args(args)
                .env("PHANTOM_FRACTAL_DATA", fixture.data.path())
                .current_dir(fixture.repo.path())
                .output()
                .expect("run");
            assert_eq!(out.status.code(), Some(1), "{bin} {args:?}");
            assert!(out.stdout.is_empty(), "{bin} {args:?} wrote stdout");
            assert!(!out.stderr.is_empty(), "{bin} {args:?} wrote no error");
        }
    }
}

#[test]
fn lookup_errors_exit_as_callers() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let (code, env) = fixture.run(bin, &["impact", "nothing_by_this_name"]);
        assert_eq!(code, Some(3), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "not_found", "{env}");

        let (code, env) = fixture.run(bin, &["impact", "root_fn", "--repo", "missing"]);
        assert_eq!(code, Some(3), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "not_found", "{env}");

        // `helper` is both the inline `cfg(test)` module's and `tests.rs`'s.
        let (code, env) = fixture.run(bin, &["impact", "helper"]);
        assert_eq!(code, Some(6), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "ambiguous", "{env}");
        assert_eq!(env["data"]["total"], 2, "{env}");
        let candidates = env["data"]["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), 2, "{env}");
    }
}

#[test]
fn without_a_current_index_it_is_no_index() {
    for bin in BINS {
        let fixture = Fixture {
            repo: TempDir::new("repo"),
            data: TempDir::new("data"),
        };
        copy_tree(Path::new(FIXTURE), fixture.repo.path());
        let (code, env) = fixture.run(bin, &["impact", "root_fn"]);
        assert_eq!(code, Some(2), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "no_index", "{env}");
        assert!(
            fs::read_dir(fixture.data.path())
                .expect("data dir")
                .next()
                .is_none(),
            "{bin} wrote to the data directory"
        );
    }
    let fixture = Fixture::indexed();
    Store::open(&find_database(fixture.data.path()))
        .expect("open the index")
        .set_extractor_version(4)
        .expect("stamp an older extractor");
    for bin in BINS {
        let (code, env) = fixture.run(bin, &["impact", "root_fn"]);
        assert_eq!(code, Some(2), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "no_index", "{env}");
    }
}

#[test]
fn the_global_json_flag_changes_nothing() {
    let fixture = Fixture::indexed();
    for bin in BINS {
        let plain = fixture.run(bin, &["impact", "area"]);
        for args in [
            ["--json", "impact", "area"],
            ["impact", "--json", "area"],
            ["impact", "area", "--json"],
        ] {
            assert_eq!(fixture.run(bin, &args), plain, "{bin} {args:?}");
        }
    }
}
