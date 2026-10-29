//! `show` against temp copies of `fixtures/detail` and `fixtures/impact`,
//! and against small generated trees for the caps and the source
//! statuses, with the data directory in a second temp directory
//! (SPEC-w6 G1b).

use std::fmt::Write as _;
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

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const DETAIL_EXPECTED: &str = include_str!("../../../fixtures/detail/EXPECTED.md");

/// `data` keys in SPEC order, without and with `--source`.
const DATA_KEYS: [&str; 5] = ["symbol", "members", "callers", "callees", "macro_args"];

/// `symbol` keys in SPEC order; a member has the same keys.
const SYMBOL_KEYS: [&str; 11] = [
    "name",
    "qual_name",
    "kind",
    "visibility",
    "path",
    "line",
    "end_line",
    "signature",
    "doc_summary",
    "is_test",
    "stable_key",
];

const SOURCE_KEYS: [&str; 4] = ["status", "start_line", "end_line", "text"];

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-show-{label}-{}-{}",
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

/// Every string in `value` at any depth, except the source `text`, which
/// is file content.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| key.as_str() != "text")
            .flat_map(|(_, value)| strings(value))
            .collect(),
        _ => Vec::new(),
    }
}

/// The keys of the object that starts at `at` in `raw`, at its own depth,
/// in the order they were printed.
fn raw_keys(raw: &str, at: usize) -> Vec<String> {
    let bytes = raw.as_bytes();
    assert_eq!(bytes[at], b'{', "no object at {at}: {raw}");
    let (mut depth, mut keys, mut i) = (0usize, Vec::new(), at);
    let mut expect_key = false;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => {
                depth += 1;
                expect_key = bytes[i] == b'{' && depth == 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b',' if depth == 1 => expect_key = true,
            b'"' => {
                let start = i + 1;
                i += 1;
                while bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if expect_key && depth == 1 {
                    keys.push(raw[start..i].to_owned());
                    expect_key = false;
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// Where the value of the first `"key":` at or after `from` starts.
fn value_at(raw: &str, key: &str, from: usize) -> usize {
    let needle = format!("\"{key}\":");
    from + raw[from..]
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key}"))
        + needle.len()
}

struct Fixture {
    repo: TempDir,
    data: TempDir,
}

impl Fixture {
    /// A copy of `fixtures/NAME`, indexed.
    fn of(name: &str) -> Self {
        let fixture = Self::empty();
        copy_tree(&Path::new(FIXTURES).join(name), fixture.repo.path());
        fixture.index();
        fixture
    }

    /// An empty repo, not yet indexed.
    fn empty() -> Self {
        Self {
            repo: TempDir::new("repo"),
            data: TempDir::new("data"),
        }
    }

    /// A generated package with `src/lib.rs` and `files`, indexed.
    fn generated(files: &[(&str, String)]) -> Self {
        let fixture = Self::empty();
        let root = fixture.repo.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::create_dir_all(root.join("src")).expect("create src");
        for (path, text) in files {
            fs::write(root.join(path), text).expect("write source");
        }
        fixture.index();
        fixture
    }

    fn index(&self) {
        let repo = self.repo.path().to_str().expect("utf-8 temp path");
        let (code, _, env) = self.run(BINS[1], &["index", repo]);
        assert_eq!(code, Some(0), "index failed: {env}");
        assert_eq!(env["data"]["skipped"], 0, "{env}");
    }

    fn store(&self) -> Store {
        Store::open(&find_database(self.data.path())).expect("open the index")
    }

    /// Runs `bin` in the repo and checks that nothing it printed names
    /// either temp directory or any absolute path. The exit code, the raw
    /// stdout, and the envelope.
    fn run(&self, bin: &str, args: &[&str]) -> (Option<i32>, String, Value) {
        let out = Command::new(bin)
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .current_dir(self.repo.path())
            .output()
            .unwrap_or_else(|err| panic!("cannot run {bin}: {err}"));
        let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
        let text = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
        for dir in [self.repo.path(), self.data.path()] {
            let dir = dir.to_str().expect("utf-8");
            assert!(!text.contains(dir), "{bin} {args:?} printed {dir}: {text}");
        }
        let env: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|err| panic!("stdout is not one JSON envelope ({err}): {text}"));
        for value in strings(&env) {
            assert!(
                !Path::new(value).is_absolute(),
                "{bin} {args:?} printed an absolute path {value:?}: {text}"
            );
        }
        assert_eq!(env["schema"], 1, "{bin} {args:?}: {env}");
        (out.status.code(), stdout, env)
    }

    /// `show ARGS` that must succeed: the raw line and its `data`.
    fn show(&self, bin: &str, args: &[&str]) -> (String, Value) {
        let args: Vec<&str> = ["show"].iter().chain(args).copied().collect();
        let (code, raw, env) = self.run(bin, &args);
        assert_eq!(code, Some(0), "{bin} {args:?}: {env}");
        assert_eq!(env["ok"], true, "{bin} {args:?}: {env}");
        assert_eq!(env["error"], Value::Null, "{bin} {args:?}: {env}");
        (raw, env["data"].clone())
    }

    /// `show` of the symbol declared at `path:line` named `query`: by name,
    /// or through its candidate's stable key when the name is ambiguous.
    fn show_at(&self, query: &str, path: &str, line: i64, extra: &[&str]) -> Value {
        let args: Vec<&str> = ["show", query].iter().chain(extra).copied().collect();
        let (code, _, env) = self.run(BINS[0], &args);
        match code {
            Some(0) => env["data"].clone(),
            Some(6) => {
                let key = env["data"]["candidates"]
                    .as_array()
                    .expect("candidates")
                    .iter()
                    .find(|candidate| candidate["path"] == path && candidate["line"] == line)
                    .unwrap_or_else(|| panic!("{query} at {path}:{line} not listed: {env}"))
                    ["stable_key"]
                    .as_str()
                    .expect("stable_key")
                    .to_owned();
                let args: Vec<&str> = ["--id", key.as_str()]
                    .iter()
                    .chain(extra)
                    .copied()
                    .collect();
                self.show(BINS[0], &args).1
            }
            _ => panic!("show {query}: {env}"),
        }
    }
}

/// One `### KIND NAME` row of `fixtures/detail/EXPECTED.md`.
struct Row {
    path: String,
    kind: String,
    name: String,
    line: i64,
    end_line: i64,
    signature: String,
    doc_summary: Option<String>,
}

fn detail_rows() -> Vec<Row> {
    let (mut rows, mut path): (Vec<Row>, String) = (Vec::new(), String::new());
    for line in DETAIL_EXPECTED.lines() {
        if let Some(file) = line.strip_prefix("## File ") {
            path = file.trim().to_owned();
        } else if let Some(heading) = line.strip_prefix("### ") {
            let (kind, name) = heading.split_once(' ').expect("KIND NAME");
            rows.push(Row {
                path: path.clone(),
                kind: kind.to_owned(),
                name: name.to_owned(),
                line: 0,
                end_line: 0,
                signature: String::new(),
                doc_summary: None,
            });
        } else if let (Some(row), Some((key, value))) = (
            rows.last_mut(),
            line.strip_prefix("- ")
                .and_then(|entry| entry.split_once(": ")),
        ) {
            match key {
                "line" => row.line = value.parse().expect("line"),
                "end_line" => row.end_line = value.parse().expect("end_line"),
                "signature" => row.signature = value.to_owned(),
                "doc_summary" => row.doc_summary = (value != "none").then(|| value.to_owned()),
                _ => {}
            }
        }
    }
    rows
}

/// Lines `start` to `end` of `text`, 1-based, each with its line ending.
fn lines_of(text: &str, start: i64, end: i64) -> String {
    text.split_inclusive('\n')
        .skip(usize::try_from(start - 1).expect("start"))
        .take(usize::try_from(end - start + 1).expect("range"))
        .collect()
}

#[test]
fn data_and_symbol_keys_are_in_spec_order() {
    let fixture = Fixture::of("detail");
    for bin in BINS {
        for source in [false, true] {
            let mut args = vec!["Grid"];
            if source {
                args.push("--source");
            }
            let (raw, data) = fixture.show(bin, &args);
            let data_at = value_at(&raw, "data", 0);
            let mut want: Vec<&str> = DATA_KEYS.to_vec();
            if source {
                want.push("source");
            }
            assert_eq!(raw_keys(&raw, data_at), want, "{bin} {args:?}: {raw}");
            assert_eq!(
                raw_keys(&raw, value_at(&raw, "symbol", data_at)),
                SYMBOL_KEYS,
                "{raw}"
            );
            let members_at = value_at(&raw, "members", data_at);
            assert_eq!(raw_keys(&raw, members_at), ["hits", "truncated"], "{raw}");
            assert_eq!(
                raw_keys(&raw, value_at(&raw, "hits", members_at) + 1),
                SYMBOL_KEYS,
                "a member has the symbol shape: {raw}"
            );
            let callers_at = value_at(&raw, "callers", members_at);
            assert_eq!(
                raw_keys(&raw, callers_at),
                ["count", "hits", "truncated"],
                "{raw}"
            );
            let callees_at = value_at(&raw, "callees", callers_at);
            assert_eq!(
                raw_keys(&raw, callees_at),
                ["count", "hits", "truncated", "unresolved_calls"],
                "{raw}"
            );
            if source {
                assert_eq!(
                    raw_keys(&raw, value_at(&raw, "source", callees_at)),
                    SOURCE_KEYS,
                    "{raw}"
                );
            } else {
                assert!(data.get("source").is_none(), "{raw}");
            }
        }
    }
}

#[test]
fn every_detail_row_shows_its_stored_detail() {
    let fixture = Fixture::of("detail");
    let rows = detail_rows();
    assert!(rows.len() >= 12, "EXPECTED.md has {} rows", rows.len());
    for row in rows {
        let data = fixture.show_at(&row.name, &row.path, row.line, &[]);
        let symbol = &data["symbol"];
        let what = format!("{} {}", row.kind, row.name);
        let bare = row.name.rsplit("::").next().expect("name");
        assert_eq!(symbol["name"], bare, "{what}: {symbol}");
        assert!(
            symbol["qual_name"]
                .as_str()
                .is_some_and(|qual| qual.ends_with(&row.name)),
            "{what}: {symbol}"
        );
        assert_eq!(symbol["kind"], row.kind, "{what}");
        assert_eq!(symbol["path"], row.path, "{what}");
        assert_eq!(symbol["line"], row.line, "{what}");
        assert_eq!(symbol["end_line"], row.end_line, "{what}");
        assert_eq!(symbol["signature"], row.signature, "{what}");
        match &row.doc_summary {
            Some(doc) => assert_eq!(symbol["doc_summary"], doc.as_str(), "{what}"),
            None => assert_eq!(symbol["doc_summary"], Value::Null, "{what}"),
        }
        assert!(symbol["is_test"].is_boolean(), "{what}");
        // By key, the answer is the same line.
        let key = symbol["stable_key"].as_str().expect("stable_key");
        assert_eq!(fixture.show(BINS[1], &["--id", key]).1, data, "{what}");
    }
    let grid = fixture.show(BINS[0], &["Grid"]).1;
    let members: Vec<&Value> = grid["members"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|member| &member["name"])
        .collect();
    assert_eq!(members, ["filled", "len"], "{grid}");
    assert_eq!(grid["members"]["truncated"], false, "{grid}");
    assert_eq!(grid["symbol"]["visibility"], "pub", "{grid}");
}

/// For every stored symbol of both fixtures, `show --id` agrees with the
/// store's detail, members, edge counts, and `macro_args`, and its edge
/// hits are the first of what `callers` and `callees` print.
#[test]
fn show_agrees_with_the_store_and_the_call_commands() {
    for name in ["detail", "impact"] {
        let fixture = Fixture::of(name);
        let store = fixture.store();
        let input = store.resolve_input().expect("resolve input");
        let mut seen_members = false;
        let mut seen_macro_args = false;
        for symbol in &input.symbols {
            let detail = store
                .symbol_detail(symbol.id)
                .expect("detail")
                .expect("row");
            let data = fixture.show(BINS[0], &["--id", &detail.stable_key]).1;
            let what = format!("{name} {}", detail.stable_key);
            let shown = &data["symbol"];
            assert_eq!(shown["stable_key"], detail.stable_key.as_str(), "{what}");
            assert_eq!(shown["name"], detail.name.as_str(), "{what}");
            assert_eq!(shown["kind"], detail.kind.as_str(), "{what}");
            assert_eq!(shown["path"], detail.path.as_str(), "{what}");
            assert_eq!(
                shown["line"],
                serde_json::json!(detail.start_line),
                "{what}"
            );
            assert_eq!(
                shown["end_line"],
                serde_json::json!(detail.end_line),
                "{what}"
            );
            assert_eq!(
                shown["signature"],
                serde_json::json!(detail.signature),
                "{what}"
            );
            assert_eq!(
                shown["doc_summary"],
                serde_json::json!(detail.doc_summary),
                "{what}"
            );
            assert_eq!(shown["is_test"], detail.is_test, "{what}");

            let members = store.members(symbol.id).expect("members");
            seen_members |= !members.is_empty();
            let keys: Vec<&str> = data["members"]["hits"]
                .as_array()
                .expect("member hits")
                .iter()
                .map(|member| member["stable_key"].as_str().expect("key"))
                .collect();
            let want: Vec<&str> = members
                .iter()
                .map(|member| member.stable_key.as_str())
                .collect();
            assert_eq!(keys, want, "{what}");

            let callers = store.callers(symbol.id).expect("callers").len();
            let callees = store.callees(symbol.id).expect("callees").len();
            assert_eq!(data["callers"]["count"], callers, "{what}");
            assert_eq!(data["callees"]["count"], callees, "{what}");
            assert_eq!(
                data["callees"]["unresolved_calls"],
                store.unresolved_calls(symbol.id).expect("unresolved"),
                "{what}"
            );
            let macro_args = store.macro_args_named(&detail.name).expect("macro args");
            seen_macro_args |= macro_args > 0;
            assert_eq!(data["macro_args"], macro_args, "{what}");

            // Where the qualified name names one symbol, `callers` and
            // `callees` print the same hits, uncapped below 20.
            let qual = shown["qual_name"].as_str().expect("qual_name");
            for side in ["callers", "callees"] {
                let (code, _, env) = fixture.run(BINS[1], &[side, qual]);
                if code != Some(0) || env["data"]["symbol"]["path"] != shown["path"] {
                    continue;
                }
                let full = env["data"]["hits"].as_array().expect("hits");
                let shown_hits = data[side]["hits"].as_array().expect("hits");
                let cap = full.len().min(20);
                assert_eq!(shown_hits.as_slice(), &full[..cap], "{what} {side}");
                assert_eq!(data[side]["truncated"], full.len() > 20, "{what} {side}");
            }
        }
        assert!(seen_members || name == "impact", "{name} has no members");
        assert!(
            seen_macro_args || name == "detail",
            "{name} has no macro args"
        );
    }
}

#[test]
fn members_callers_and_callees_are_capped() {
    let mut lib = String::from("pub struct Wide;\n\nimpl Wide {\n");
    for n in 0..105 {
        writeln!(lib, "    pub fn m{n:03}(&self) {{}}").expect("write");
    }
    lib.push_str("}\n\npub fn target() {}\n\n");
    for n in 0..25 {
        writeln!(lib, "pub fn c{n:02}() {{\n    target();\n}}").expect("write");
    }
    lib.push_str("\npub fn fan() {\n");
    for n in 0..25 {
        writeln!(lib, "    c{n:02}();").expect("write");
    }
    lib.push_str("}\n");
    let fixture = Fixture::generated(&[("src/lib.rs", lib)]);
    for bin in BINS {
        let wide = fixture.show(bin, &["Wide"]).1;
        assert_eq!(wide["members"]["hits"].as_array().map(Vec::len), Some(100));
        assert_eq!(wide["members"]["truncated"], true);
        assert_eq!(wide["members"]["hits"][0]["name"], "m000");
        assert_eq!(wide["members"]["hits"][99]["name"], "m099");

        let target = fixture.show(bin, &["target"]).1;
        assert_eq!(target["callers"]["count"], 25, "{target}");
        assert_eq!(target["callers"]["hits"].as_array().map(Vec::len), Some(20));
        assert_eq!(target["callers"]["truncated"], true);
        assert_eq!(target["members"]["hits"], serde_json::json!([]));
        assert_eq!(target["members"]["truncated"], false);

        let fan = fixture.show(bin, &["fan"]).1;
        assert_eq!(fan["callees"]["count"], 25, "{fan}");
        assert_eq!(fan["callees"]["hits"].as_array().map(Vec::len), Some(20));
        assert_eq!(fan["callees"]["truncated"], true);
        assert_eq!(fan["callees"]["unresolved_calls"], 0);
        assert_eq!(fan["callers"]["count"], 0);
        assert_eq!(fan["callers"]["truncated"], false);
    }
}

#[test]
fn exits_are_usage_no_index_not_found_and_ambiguous() {
    let usage: [&[&str]; 6] = [
        &["show"],
        &["show", "--repo", "."],
        &["show", "Grid", "--id", "src/shapes.rs#Grid#struct#0"],
        &["show", "Grid", "extra"],
        &["show", "--id"],
        &["show", "Grid", "--source", "yes"],
    ];
    let fixture = Fixture::of("detail");
    for bin in BINS {
        for args in usage {
            let out = Command::new(bin)
                .args(args)
                .env("PHANTOM_FRACTAL_DATA", fixture.data.path())
                .current_dir(fixture.repo.path())
                .output()
                .expect("run");
            assert_eq!(out.status.code(), Some(1), "{bin} {args:?}");
            assert!(out.stdout.is_empty(), "{bin} {args:?} wrote stdout");
            assert!(!out.stderr.is_empty(), "{bin} {args:?} wrote no error");
        }
        for args in [
            &["show", "nosuch"][..],
            &["show", "--id", "src/lib.rs#nosuch#function#0"],
        ] {
            let (code, _, env) = fixture.run(bin, args);
            assert_eq!(code, Some(3), "{bin} {args:?}: {env}");
            assert_eq!(env["error"]["kind"], "not_found", "{env}");
            assert_eq!(env["data"], Value::Null, "{env}");
        }
    }

    let unindexed = Fixture::empty();
    for bin in BINS {
        let (code, _, env) = unindexed.run(bin, &["show", "anything"]);
        assert_eq!(code, Some(2), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "no_index", "{env}");
    }
    let stale = Fixture::of("detail");
    stale
        .store()
        .set_extractor_version(5)
        .expect("stamp an older extractor");
    let (code, _, env) = stale.run(BINS[0], &["show", "Grid"]);
    assert_eq!(code, Some(2), "{env}");

    let mut lib = String::new();
    for n in 0..12 {
        writeln!(lib, "pub mod m{n:02} {{\n    pub fn dup() {{}}\n}}").expect("write");
    }
    let many = Fixture::generated(&[("src/lib.rs", lib)]);
    for bin in BINS {
        let (code, _, env) = many.run(bin, &["show", "dup"]);
        assert_eq!(code, Some(6), "{bin}: {env}");
        assert_eq!(env["error"]["kind"], "ambiguous", "{env}");
        assert_eq!(env["data"]["total"], 12, "{env}");
        let candidates = env["data"]["candidates"].as_array().expect("candidates");
        assert_eq!(candidates.len(), 10, "{env}");
        for candidate in candidates {
            let key = candidate["stable_key"].as_str().expect("stable_key");
            let shown = many.show(bin, &["--id", key]).1;
            assert_eq!(shown["symbol"]["line"], candidate["line"], "{key}");
            assert_eq!(shown["symbol"]["qual_name"], candidate["name"], "{key}");
        }
        // `callers` keeps its candidates without keys.
        let (code, _, env) = many.run(bin, &["callers", "dup"]);
        assert_eq!(code, Some(6), "{env}");
        assert!(
            env["data"]["candidates"][0].get("stable_key").is_none(),
            "{env}"
        );
    }
}

/// Needs symlinks for the out-of-root and in-root cases.
#[cfg(unix)]
#[test]
fn source_is_ok_stale_too_large_or_unreadable() {
    let body = |name: &str, lines: usize| {
        let mut text = format!("pub fn {name}() {{\n");
        for _ in 2..lines {
            text.push_str("    let _ = 0;\n");
        }
        text.push_str("}\n");
        text
    };
    let heavy = format!(
        "pub fn heavy() {{\n{}}}\n",
        format!("    let _ = \"{}\";\n", "x".repeat(4000)).repeat(9)
    );
    let modules = [
        "plain", "lines400", "lines401", "heavy", "edited", "gone", "binary", "dir", "out",
        "inside",
    ];
    let lib: String = modules.iter().map(|m| format!("pub mod {m};\n")).collect();
    let plain = "/// Seven.\npub fn plain() -> u32 {\n    // source-marker\n    7\n}\n".to_owned();
    let files = [
        ("src/lib.rs", lib),
        ("src/plain.rs", plain.clone()),
        ("src/lines400.rs", body("lines400", 400)),
        ("src/lines401.rs", body("lines401", 401)),
        ("src/heavy.rs", heavy),
        ("src/edited.rs", body("edited", 3)),
        ("src/gone.rs", body("gone", 3)),
        ("src/binary.rs", body("binary", 3)),
        ("src/dir.rs", body("dir", 3)),
        ("src/out.rs", body("out", 3)),
        ("src/inside.rs", body("inside", 3)),
    ];
    let fixture = Fixture::generated(&files);
    let root = fixture.repo.path();
    let outside = TempDir::new("outside");

    // After the index: change, remove, or replace each file.
    fs::write(
        root.join("src/edited.rs"),
        format!("{}// edited\n", body("edited", 3)),
    )
    .expect("edit");
    fs::remove_file(root.join("src/gone.rs")).expect("remove");
    fs::write(
        root.join("src/binary.rs"),
        b"pub fn binary() {}\n\xff\xfe\n",
    )
    .expect("binary");
    fs::remove_file(root.join("src/dir.rs")).expect("remove");
    fs::create_dir(root.join("src/dir.rs")).expect("dir");
    // Same bytes, so only the root check can refuse them.
    fs::write(outside.path().join("out.rs"), body("out", 3)).expect("outside copy");
    fs::remove_file(root.join("src/out.rs")).expect("remove");
    symlink(&outside.path().join("out.rs"), &root.join("src/out.rs"));
    fs::create_dir(root.join("kept")).expect("kept dir");
    fs::write(root.join("kept/inside.rs"), body("inside", 3)).expect("inside copy");
    fs::remove_file(root.join("src/inside.rs")).expect("remove");
    symlink(&root.join("kept/inside.rs"), &root.join("src/inside.rs"));

    let cases = [
        ("plain", "ok"),
        ("lines400", "ok"),
        ("lines401", "too_large"),
        ("heavy", "too_large"),
        ("edited", "stale"),
        ("gone", "unreadable"),
        ("binary", "unreadable"),
        ("dir", "unreadable"),
        ("out", "unreadable"),
        ("inside", "ok"),
    ];
    for bin in BINS {
        for (name, status) in cases {
            // Each module shares its function's name, so qualify it.
            let query = format!("{name}::{name}");
            let (raw, data) = fixture.show(bin, &[&query, "--source"]);
            let source = &data["source"];
            assert_eq!(source["status"], status, "{bin} {name}: {source}");
            assert_eq!(source["start_line"], data["symbol"]["line"], "{name}");
            assert_eq!(source["end_line"], data["symbol"]["end_line"], "{name}");
            if status == "ok" {
                let file = fs::read_to_string(root.join(format!("src/{name}.rs"))).expect("read");
                let start = source["start_line"].as_i64().expect("start");
                let end = source["end_line"].as_i64().expect("end");
                assert_eq!(source["text"], lines_of(&file, start, end), "{name}");
            } else {
                assert_eq!(source["text"], Value::Null, "{name}: {source}");
                assert!(!raw.contains("let _ = 0;"), "{name} leaked source: {raw}");
            }
        }
        let (_, data) = fixture.show(bin, &["plain::plain", "--source"]);
        assert_eq!(
            data["source"]["text"],
            "pub fn plain() -> u32 {\n    // source-marker\n    7\n}\n"
        );
        let (raw, data) = fixture.show(bin, &["plain::plain"]);
        assert!(data.get("source").is_none(), "{raw}");
        assert!(
            !raw.contains("source-marker"),
            "source without --source: {raw}"
        );
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}
