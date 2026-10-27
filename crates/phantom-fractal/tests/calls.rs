//! `callers` and `callees` against a temp copy of `fixtures/resolve`, whose
//! edges `fixtures/resolve/EXPECTED.md` lists, and against generated trees
//! for the caps. The data directory is a second temp directory.

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

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve");

const INDEX_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/index/repo");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-calls-{label}-{}-{}",
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

struct Fixture {
    repo: TempDir,
    data: TempDir,
}

impl Fixture {
    /// A copy of `fixtures/resolve`, indexed.
    fn resolve() -> Self {
        Self::of(FIXTURE)
    }

    /// A copy of the fixture tree at `source`, indexed.
    fn of(source: &str) -> Self {
        let fixture = Self::empty();
        copy_tree(Path::new(source), fixture.repo.path());
        fixture.index();
        fixture
    }

    fn empty() -> Self {
        Self {
            repo: TempDir::new("repo"),
            data: TempDir::new("data"),
        }
    }

    fn index(&self) {
        let repo = self.repo.path().to_str().expect("utf-8 temp path");
        let (code, env) = self.run(BINS[1], &["index", repo]);
        assert_eq!(code, Some(0), "index failed: {env}");
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

    fn calls(&self, bin: &str, command: &str, symbol: &str) -> (Option<i32>, Value) {
        self.run(bin, &[command, symbol])
    }

    /// Line `line` of the repo file `path`.
    fn source_line(&self, path: &str, line: u64) -> String {
        let text = fs::read_to_string(self.repo.path().join(path)).expect("read source");
        let index = usize::try_from(line).expect("line fits") - 1;
        text.lines()
            .nth(index)
            .unwrap_or_else(|| panic!("{path} has no line {line}"))
            .to_owned()
    }
}

fn confidence(reason: &str) -> f64 {
    match reason {
        "path-exact" => 1.0,
        "local" | "use-alias" => 0.95,
        "assoc" => 0.9,
        other => panic!("unknown reason {other}"),
    }
}

/// The hits of a successful envelope as sorted `(name, reason)` pairs,
/// after checking their order, confidences, and call sites.
fn hit_set(fixture: &Fixture, env: &Value, callee: Option<&str>) -> Vec<(String, String)> {
    assert_eq!(env["ok"], true, "{env}");
    assert_eq!(env["data"]["truncated"], false, "{env}");
    let hits = env["data"]["hits"].as_array().expect("data.hits is a list");
    let sites: Vec<(&str, u64)> = hits
        .iter()
        .map(|hit| {
            (
                hit["path"].as_str().expect("path"),
                hit["line"].as_u64().expect("line"),
            )
        })
        .collect();
    assert!(sites.is_sorted(), "hits are not by path, then line: {env}");
    let mut set = Vec::new();
    for hit in hits {
        let name = hit["name"].as_str().expect("name");
        let reason = hit["reason"].as_str().expect("reason");
        assert_eq!(hit["confidence"], confidence(reason), "{hit}");
        assert!(name.starts_with("crate::"), "{hit}");
        assert!(hit["symbol_path"].as_str().is_some(), "{hit}");
        // The call site names the callee, unless a `use … as` renamed it.
        let site = fixture.source_line(
            hit["path"].as_str().expect("path"),
            hit["line"].as_u64().expect("line"),
        );
        let called = callee.unwrap_or(name);
        let last = called.rsplit("::").next().expect("a segment");
        if reason != "use-alias" {
            assert!(site.contains(&format!("{last}(")), "{hit} at {site:?}");
        }
        set.push((name.to_owned(), reason.to_owned()));
    }
    set.sort();
    set
}

/// Expected hits as `(qualified name, reason)`.
type Expected = &'static [(&'static str, &'static str)];

fn pairs(expected: Expected) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = expected
        .iter()
        .map(|(name, reason)| ((*name).to_owned(), (*reason).to_owned()))
        .collect();
    out.sort();
    out
}

/// Every unambiguous caller and callee of `EXPECTED.md`, by the names the
/// CLI prints.
#[test]
fn callers_and_callees_return_the_expected_sets() {
    let fixture = Fixture::resolve();
    let callers: [(&str, &str, Expected); 7] = [
        (
            "local_fn",
            "crate::local_fn",
            &[
                ("crate::entry", "path-exact"),
                ("crate::entry", "local"),
                ("crate::tests::check", "path-exact"),
                ("crate::util::helper", "path-exact"),
                ("crate::placed::from_path", "path-exact"),
            ],
        ),
        (
            "util::helper",
            "crate::util::helper",
            &[
                ("crate::entry", "path-exact"),
                ("crate::entry", "local"),
                ("crate::entry", "use-alias"),
                ("crate::entry", "use-alias"),
            ],
        ),
        (
            "crate::tricky::other",
            "crate::tricky::other",
            &[
                ("crate::tricky::Guest::greet", "local"),
                ("crate::tricky::inner", "local"),
                ("crate::tricky::twin", "local"),
            ],
        ),
        (
            "tricky::target",
            "crate::tricky::target",
            &[("crate::tricky::twin", "local")],
        ),
        (
            "crate::helper",
            "crate::helper",
            &[("crate::main", "local")],
        ),
        (
            "radius",
            "crate::shapes::circle::radius",
            &[("crate::shapes::area", "local")],
        ),
        ("entry", "crate::entry", &[]),
    ];
    for bin in BINS {
        for (query, name, expected) in callers {
            let (code, env) = fixture.calls(bin, "callers", query);
            assert_eq!(code, Some(0), "{bin} callers {query}: {env}");
            assert_eq!(env["data"]["symbol"]["name"], name, "{env}");
            assert!(env["data"].get("unresolved_calls").is_none(), "{env}");
            let got = hit_set(&fixture, &env, Some(name));
            assert_eq!(got, pairs(expected), "{bin} callers {query}");
        }
    }

    let callees: [(&str, Expected, u64); 8] = [
        (
            "entry",
            &[
                ("crate::util::helper", "path-exact"),
                ("crate::local_fn", "path-exact"),
                ("crate::local_fn", "local"),
                ("crate::util::helper", "local"),
                ("crate::util::helper", "use-alias"),
                ("crate::util::helper", "use-alias"),
                ("crate::Counter::new", "assoc"),
                ("crate::placed::from_path", "local"),
                ("crate::Counter::new", "assoc"),
            ],
            8,
        ),
        ("tests::check", &[("crate::local_fn", "path-exact")], 1),
        (
            "util::helper",
            &[
                ("crate::util::deep::sink", "local"),
                ("crate::local_fn", "path-exact"),
            ],
            0,
        ),
        ("sink", &[("crate::shapes::area", "path-exact")], 0),
        ("area", &[("crate::shapes::circle::radius", "local")], 0),
        ("from_path", &[("crate::local_fn", "path-exact")], 0),
        ("cli::run", &[("crate::cli::banner", "local")], 0),
        ("Guest::greet", &[("crate::tricky::other", "local")], 0),
    ];
    for bin in BINS {
        for (query, expected, unresolved) in callees {
            let (code, env) = fixture.calls(bin, "callees", query);
            assert_eq!(code, Some(0), "{bin} callees {query}: {env}");
            let symbol = &env["data"]["symbol"];
            let path = symbol["path"].as_str().expect("symbol path");
            for hit in env["data"]["hits"].as_array().expect("hits") {
                assert_eq!(
                    hit["path"], path,
                    "a callee's call site is in {path}: {hit}"
                );
            }
            assert_eq!(
                env["data"]["unresolved_calls"], unresolved,
                "{bin} callees {query}: {env}"
            );
            let got = hit_set(&fixture, &env, None);
            assert_eq!(got, pairs(expected), "{bin} callees {query}");
        }
    }
}

#[test]
fn a_symbol_reports_its_qualified_name_kind_path_and_line() {
    let fixture = Fixture::resolve();
    let (code, env) = fixture.calls(BINS[1], "callees", "crate::util::helper");
    assert_eq!(code, Some(0), "{env}");
    let symbol = &env["data"]["symbol"];
    assert_eq!(symbol["name"], "crate::util::helper", "{env}");
    assert_eq!(symbol["kind"], "function", "{env}");
    assert_eq!(symbol["path"], "src/util.rs", "{env}");
    assert_eq!(symbol["line"], 5, "{env}");
    let hit = &env["data"]["hits"][0];
    assert_eq!(hit["name"], "crate::util::deep::sink", "{env}");
    assert_eq!(hit["kind"], "function", "{env}");
    assert_eq!(hit["symbol_path"], "src/util/deep.rs", "{env}");
}

#[test]
fn several_matches_are_ambiguous_with_candidates() {
    let fixture = Fixture::resolve();
    let cases: [(&str, &[&str]); 3] = [
        ("helper", &["crate::helper", "crate::util::helper"]),
        ("main", &["crate::main", "crate::main"]),
        (
            "Counter::new",
            &["crate::Counter::new", "crate::Counter::new"],
        ),
    ];
    for bin in BINS {
        for command in ["callers", "callees"] {
            for (query, names) in cases {
                let (code, env) = fixture.calls(bin, command, query);
                assert_eq!(code, Some(6), "{bin} {command} {query}: {env}");
                assert_eq!(env["ok"], false, "{env}");
                assert_eq!(env["error"]["kind"], "ambiguous", "{env}");
                assert_eq!(env["error"]["exit"], 6, "{env}");
                assert_eq!(env["data"]["total"], names.len(), "{env}");
                let candidates = env["data"]["candidates"].as_array().expect("candidates");
                let mut got: Vec<&str> = candidates
                    .iter()
                    .map(|candidate| candidate["name"].as_str().expect("name"))
                    .collect();
                got.sort_unstable();
                assert_eq!(got, names, "{env}");
                let message = env["error"]["message"].as_str().expect("message");
                for candidate in candidates {
                    let path = candidate["path"].as_str().expect("path");
                    assert!(message.contains(path), "{message}");
                }
            }
        }
    }
}

#[test]
fn an_unknown_symbol_is_not_found() {
    let fixture = Fixture::resolve();
    let absent = [
        "no_such_symbol",
        "util::nothing",
        "shapes::helper",
        "crate::util::deep::helper",
        "Helper",
        "::helper",
        "helper::",
        "",
        "helper' OR '1'='1",
        "\"",
    ];
    for bin in BINS {
        for command in ["callers", "callees"] {
            for query in absent {
                let (code, env) = fixture.calls(bin, command, query);
                assert_eq!(code, Some(3), "{bin} {command} {query:?}: {env}");
                assert_eq!(env["ok"], false, "{env}");
                assert_eq!(env["error"]["kind"], "not_found", "{env}");
                assert_eq!(env["error"]["exit"], 3, "{env}");
            }
        }
    }
}

#[test]
fn without_an_index_it_is_no_index_and_creates_nothing() {
    let fixture = Fixture::empty();
    copy_tree(Path::new(FIXTURE), fixture.repo.path());
    for bin in BINS {
        for command in ["callers", "callees"] {
            let (code, env) = fixture.calls(bin, command, "local_fn");
            assert_eq!(code, Some(2), "{bin} {command}: {env}");
            assert_eq!(env["error"]["kind"], "no_index", "{env}");
        }
    }
    let created = fs::read_dir(fixture.data.path()).expect("data dir").count();
    assert_eq!(created, 0, "a lookup created files in the data directory");
}

#[test]
fn an_index_from_before_call_edges_is_no_index() {
    let fixture = Fixture::resolve();
    let database = find_database(fixture.data.path());
    Store::open(&database)
        .expect("open the index")
        .set_extractor_version(1)
        .expect("stamp version 1");
    for command in ["callers", "callees"] {
        let (code, env) = fixture.calls(BINS[0], command, "local_fn");
        assert_eq!(code, Some(2), "{command}: {env}");
        assert_eq!(env["error"]["kind"], "no_index", "{env}");
    }
}

#[test]
fn repo_flag_works_from_another_directory_and_a_missing_repo_is_not_found() {
    let fixture = Fixture::resolve();
    let repo = fixture.repo.path().to_str().expect("utf-8");
    for command in ["callers", "callees"] {
        let out = Command::new(BINS[1])
            .args([command, "util::helper", "--repo", repo])
            .env("PHANTOM_FRACTAL_DATA", fixture.data.path())
            .current_dir(fixture.data.path())
            .output()
            .expect("run fract");
        assert_eq!(out.status.code(), Some(0), "{command}");
        let env: Value = serde_json::from_slice(&out.stdout).expect("envelope");
        assert_eq!(env["data"]["symbol"]["name"], "crate::util::helper");
        assert!(!String::from_utf8_lossy(&out.stdout).contains(repo));

        let missing = fixture.repo.path().join("no-such-dir");
        let missing = missing.to_str().expect("utf-8");
        let (code, env) = fixture.run(BINS[1], &[command, "helper", "--repo", missing]);
        assert_eq!(code, Some(3), "{command}: {env}");
        assert_eq!(env["error"]["kind"], "not_found", "{env}");
    }
}

/// A package whose `target` has `count` callers and whose `hub` makes
/// `count` calls, plus `dups` functions named `dup` in inline modules.
fn plant_wide_package(root: &Path, count: usize, dups: usize) {
    let mut lib = String::from("pub fn target() {}\n\npub fn hub() {\n");
    for n in 0..count {
        lib.push_str(&format!("    f{n:03}();\n"));
    }
    lib.push_str("}\n\n");
    for n in 0..count {
        lib.push_str(&format!("pub fn c{n:03}() {{\n    target();\n}}\n\n"));
        lib.push_str(&format!("pub fn f{n:03}() {{}}\n\n"));
    }
    for n in 0..dups {
        lib.push_str(&format!(
            "pub mod m{n:02} {{\n    pub fn dup() {{}}\n}}\n\n"
        ));
    }
    fs::create_dir_all(root.join("src")).expect("create src");
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"wide\"\n").expect("write manifest");
    fs::write(root.join("src/lib.rs"), lib).expect("write lib");
}

fn wide(count: usize, dups: usize) -> Fixture {
    let fixture = Fixture::empty();
    plant_wide_package(fixture.repo.path(), count, dups);
    fixture.index();
    fixture
}

fn assert_capped(env: &Value, len: usize, truncated: bool) {
    assert_eq!(env["ok"], true, "{env}");
    let hits = env["data"]["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), len, "{env}");
    assert_eq!(env["data"]["truncated"], truncated, "{env}");
    let lines: Vec<u64> = hits
        .iter()
        .map(|hit| hit["line"].as_u64().expect("line"))
        .collect();
    assert!(lines.is_sorted(), "hits are not in line order: {lines:?}");
}

#[test]
fn over_the_cap_returns_the_first_100_and_truncated() {
    let fixture = wide(101, 0);
    for bin in BINS {
        let (code, env) = fixture.calls(bin, "callers", "target");
        assert_eq!(code, Some(0), "{bin}: {env}");
        assert_capped(&env, 100, true);
        let names: Vec<&str> = env["data"]["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .map(|hit| hit["name"].as_str().expect("name"))
            .collect();
        let expected: Vec<String> = (0..100).map(|n| format!("crate::c{n:03}")).collect();
        assert_eq!(names, expected, "{bin}: not the first 100 call sites");

        let (code, env) = fixture.calls(bin, "callees", "hub");
        assert_eq!(code, Some(0), "{bin}: {env}");
        assert_capped(&env, 100, true);
        assert_eq!(env["data"]["hits"][99]["name"], "crate::f099", "{env}");
        assert_eq!(env["data"]["unresolved_calls"], 0, "{env}");
    }
}

#[test]
fn at_the_cap_is_not_truncated() {
    let fixture = wide(100, 0);
    let (code, env) = fixture.calls(BINS[0], "callers", "target");
    assert_eq!(code, Some(0), "{env}");
    assert_capped(&env, 100, false);
    let (code, env) = fixture.calls(BINS[1], "callees", "hub");
    assert_eq!(code, Some(0), "{env}");
    assert_capped(&env, 100, false);
}

#[test]
fn ambiguous_lists_at_most_10_candidates() {
    let fixture = wide(1, 12);
    let (code, env) = fixture.calls(BINS[1], "callers", "dup");
    assert_eq!(code, Some(6), "{env}");
    assert_eq!(env["data"]["total"], 12, "{env}");
    let candidates = env["data"]["candidates"].as_array().expect("candidates");
    assert_eq!(candidates.len(), 10, "{env}");
    let message = env["error"]["message"].as_str().expect("message");
    assert!(message.contains("and 2 more"), "{message}");
    assert_eq!(message.matches("crate::m").count(), 10, "{message}");

    let (code, env) = fixture.calls(BINS[1], "callers", "m11::dup");
    assert_eq!(code, Some(0), "a qualified name picks one: {env}");
    assert_eq!(env["data"]["symbol"]["name"], "crate::m11::dup", "{env}");
}

/// The symbol a lookup named, or the envelope when it named none or many.
fn named(fixture: &Fixture, bin: &str, query: &str) -> (Option<i32>, Value) {
    let (code, env) = fixture.calls(bin, "callers", query);
    (code, env["data"]["symbol"].clone())
}

/// The module-qualified form is the file module path, with or without
/// `crate::`, joined to the in-file name. An inline module's scope is not
/// doubled, and a partial module path is not a match.
#[test]
fn module_qualified_names_name_one_symbol() {
    let index = Fixture::of(INDEX_FIXTURE);
    let resolve = Fixture::resolve();
    let found: [(&Fixture, &str, &str, &str); 8] = [
        (
            &index,
            "ledger::open_ledger",
            "crate::ledger::open_ledger",
            "src/ledger.rs",
        ),
        (
            &index,
            "crate::ledger::open_ledger",
            "crate::ledger::open_ledger",
            "src/ledger.rs",
        ),
        (
            &resolve,
            "tests::check",
            "crate::tests::check",
            "src/lib.rs",
        ),
        (
            &resolve,
            "crate::tests::check",
            "crate::tests::check",
            "src/lib.rs",
        ),
        (
            &resolve,
            "util::deep::sink",
            "crate::util::deep::sink",
            "src/util/deep.rs",
        ),
        (
            &resolve,
            "crate::util::deep::sink",
            "crate::util::deep::sink",
            "src/util/deep.rs",
        ),
        (
            &resolve,
            "tricky::Guest::greet",
            "crate::tricky::Guest::greet",
            "src/tricky.rs",
        ),
        (
            &resolve,
            "shapes::circle::radius",
            "crate::shapes::circle::radius",
            "src/shapes/circle.rs",
        ),
    ];
    let absent: [(&Fixture, &str); 5] = [
        (&resolve, "crate::tests::tests::check"),
        (&resolve, "tests::tests::check"),
        (&resolve, "deep::sink"),
        (&resolve, "crate::crate::util::helper"),
        (&index, "ledger::ledger::open_ledger"),
    ];
    for bin in BINS {
        for (fixture, query, name, path) in found {
            let (code, symbol) = named(fixture, bin, query);
            assert_eq!(code, Some(0), "{bin} {query}: {symbol}");
            assert_eq!(symbol["name"], name, "{bin} {query}");
            assert_eq!(symbol["path"], path, "{bin} {query}");
        }
        for (fixture, query) in absent {
            let (code, env) = fixture.calls(bin, "callees", query);
            assert_eq!(code, Some(3), "{bin} {query}: {env}");
            assert_eq!(env["error"]["kind"], "not_found", "{env}");
        }
    }
    let (_, symbol) = named(&index, BINS[1], "ledger::open_ledger");
    assert_eq!(symbol["kind"], "function", "{symbol}");
    assert_eq!(symbol["line"], 7, "{symbol}");
}
