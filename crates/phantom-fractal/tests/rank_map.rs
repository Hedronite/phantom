//! `index` storing the rank, and the `rank` and `map` commands
//! (SPEC-w7 G3a), on copies of `fixtures/graph` and `fixtures/impact`.
//!
//! Each command's `data` is checked against the analyze crate's own
//! `rank`, `personalised` and `render` on the same store, and every read
//! is checked to leave `graph.db` and its WAL byte for byte as they were.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use phantom_fractal_analyze::{Detail, MapRequest, Ranks, personalised, rank, render};
use phantom_fractal_store::Store;
use serde_json::Value;

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-rank-map-{label}-{}-{}",
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

/// One exit code and stdout.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The one envelope line, parsed.
    fn envelope(&self) -> Value {
        let line = self.stdout.strip_suffix('\n').expect("one line");
        assert!(!line.contains('\n'), "{line}");
        serde_json::from_str(line).expect("envelope")
    }
}

/// A copy of one fixture and a data directory to index it into.
struct World {
    tree: TempDir,
    data: TempDir,
}

impl World {
    /// `fixture` copied and indexed.
    fn new(fixture: &str) -> Self {
        let world = Self::unindexed(fixture);
        let run = world.run(BINS[0], &["index", "."]);
        assert_eq!(run.code, Some(0), "{}", run.stdout);
        world
    }

    fn unindexed(fixture: &str) -> Self {
        let world = Self {
            tree: TempDir::new("tree"),
            data: TempDir::new("data"),
        };
        copy_tree(&Path::new(FIXTURES).join(fixture), world.tree.path());
        world
    }

    /// A generated package with `src/lib.rs` and `files`, indexed.
    fn generated(files: &[(&str, &str)]) -> Self {
        let world = Self {
            tree: TempDir::new("tree"),
            data: TempDir::new("data"),
        };
        let root = world.tree.path();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"generated\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        fs::create_dir_all(root.join("src")).expect("create src");
        for (path, text) in files {
            fs::write(root.join(path), text).expect("write source");
        }
        let run = world.run(BINS[0], &["index", "."]);
        assert_eq!(run.code, Some(0), "{}", run.stdout);
        world
    }

    fn run(&self, bin: &str, args: &[&str]) -> Run {
        let output = Command::new(bin)
            .args(args)
            .current_dir(self.tree.path())
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .output()
            .expect("run binary");
        Run {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
            stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
        }
    }

    /// The one `graph.db` under the data directory.
    fn database(&self) -> PathBuf {
        let mut stack = vec![self.data.path().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read data dir") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.ends_with("graph.db") {
                    return path;
                }
            }
        }
        panic!("no graph.db under the data directory");
    }

    /// The bytes and modification time of `graph.db`, and the bytes of
    /// its WAL when there is one. A write that leaves the same bytes still
    /// moves the time: in WAL mode the header's change counter does not.
    fn bytes(&self) -> (Vec<u8>, Option<SystemTime>, Option<Vec<u8>>) {
        let database = self.database();
        let wal = database.with_file_name("graph.db-wal");
        (
            fs::read(&database).expect("read graph.db"),
            fs::metadata(&database)
                .and_then(|meta| meta.modified())
                .ok(),
            fs::read(wal).ok(),
        )
    }

    fn store(&self) -> Store {
        Store::open(&self.database()).expect("open store")
    }
}

/// The keys of the object that starts at `at` in `raw`, at its own depth,
/// in the order they were printed. A parsed `Value` sorts its keys.
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

/// A file id by its stored path.
fn file_id(store: &Store, path: &str) -> i64 {
    store
        .files()
        .expect("files")
        .into_iter()
        .find(|file| file.path == path)
        .expect("indexed file")
        .id
}

/// The first `count` scores of `ranks`, highest first, as JSON prints
/// them.
fn top(scores: &[(i64, f64)], count: usize) -> Vec<String> {
    let mut scores: Vec<f64> = scores.iter().map(|(_, score)| *score).collect();
    scores.sort_by(|a, b| b.total_cmp(a));
    scores
        .iter()
        .take(count)
        .map(|score| serde_json::to_string(score).expect("score"))
        .collect()
}

/// Every `score` in `raw`, as printed. Parsing a float back can move it by
/// one unit in the last place, so scores are compared as text.
fn printed_scores(raw: &str) -> Vec<String> {
    raw.split("\"score\":")
        .skip(1)
        .map(|rest| rest.split([',', '}']).next().expect("a number").to_owned())
        .collect()
}

#[test]
fn index_stores_the_rank_once_per_rebuild() {
    let world = World::new("graph");
    let store = world.store();
    let epoch = store.graph_epoch().expect("graph epoch");
    assert!(epoch >= 1);
    assert_eq!(store.rank_epoch().expect("rank epoch"), epoch);
    let computed = rank(&store).expect("rank");
    let stored = |store: &Store| -> Ranks {
        let mut ranks = Ranks {
            symbols: Vec::new(),
            files: Vec::new(),
            epoch: store.rank_epoch().expect("rank epoch"),
        };
        for (kind, id, score) in store.stored_rank().expect("stored rank") {
            match kind.as_str() {
                "symbol" => ranks.symbols.push((id, score)),
                "file" => ranks.files.push((id, score)),
                other => panic!("node kind {other}"),
            }
        }
        ranks
    };
    assert_eq!(stored(&store), computed);
    drop(store);

    // A pass that rebuilds nothing writes nothing, rank included.
    let before = world.bytes();
    let run = world.run(BINS[1], &["index", "."]);
    assert_eq!(run.code, Some(0));
    assert_eq!(world.bytes(), before);

    // A stale stored rank is replaced by the next pass, even one that
    // rebuilds nothing, and only that once.
    world
        .store()
        .write_rank(epoch - 1, &[], &[])
        .expect("write a stale rank");
    let run = world.run(BINS[0], &["index", "."]);
    assert_eq!(run.code, Some(0));
    assert_eq!(stored(&world.store()), computed);
    let before = world.bytes();
    world.run(BINS[0], &["index", "."]);
    assert_eq!(world.bytes(), before);

    // A changed tree rebuilds the graph, and the rank follows it.
    let lib = world.tree.path().join("kit/src/lib.rs");
    let mut text = fs::read_to_string(&lib).expect("read lib.rs");
    text.push_str("\npub fn added() -> f64 {\n    scale(1.0)\n}\n");
    fs::write(&lib, text).expect("write lib.rs");
    let run = world.run(BINS[0], &["index", "."]);
    assert_eq!(run.code, Some(0));
    let store = world.store();
    assert_eq!(store.graph_epoch().expect("graph epoch"), epoch + 1);
    assert_eq!(stored(&store), rank(&store).expect("rank"));
    assert_eq!(stored(&store).epoch, epoch + 1);
}

#[test]
fn rank_lists_the_top_symbols_or_files_as_the_analysis_ranks_them() {
    let world = World::new("graph");
    let store = world.store();
    let computed = rank(&store).expect("rank");
    for bin in BINS {
        let run = world.run(bin, &["rank"]);
        let raw = &run.stdout;
        let data_at = value_at(raw, "data", 0);
        assert_eq!(
            raw_keys(raw, data_at),
            ["kind", "epoch", "focus", "hits", "truncated"]
        );
        assert_eq!(
            raw_keys(raw, value_at(raw, "hits", data_at) + 1),
            ["name", "qual_name", "kind", "path", "line", "score"]
        );
        let env = run.envelope();
        let data = &env["data"];
        assert_eq!(data["kind"], "symbols");
        assert_eq!(data["epoch"], computed.epoch);
        assert_eq!(data["focus"], serde_json::json!([]));
        let hits = data["hits"].as_array().expect("hits");
        let want = computed.symbols.len().min(20);
        assert_eq!(hits.len(), want);
        assert_eq!(data["truncated"], computed.symbols.len() > 20);
        for hit in hits {
            assert_eq!(hit.as_object().expect("hit").len(), 6);
            assert!(
                hit["qual_name"]
                    .as_str()
                    .expect("qual_name")
                    .ends_with(hit["name"].as_str().expect("name"))
            );
        }
        assert_eq!(printed_scores(raw), top(&computed.symbols, want));

        let run = world.run(bin, &["rank", "--files", "--limit", "100"]);
        let raw = &run.stdout;
        assert_eq!(
            raw_keys(raw, value_at(raw, "hits", 0) + 1),
            ["path", "score"]
        );
        let env = run.envelope();
        let data = &env["data"];
        assert_eq!(data["kind"], "files");
        assert_eq!(data["truncated"], false);
        let hits = data["hits"].as_array().expect("hits");
        assert_eq!(hits.len(), computed.files.len());
        for hit in hits {
            assert_eq!(hit.as_object().expect("hit").len(), 2);
        }
        assert_eq!(printed_scores(raw), top(&computed.files, usize::MAX));
        // Equal scores fall back to the path.
        for pair in hits.windows(2) {
            if pair[0]["score"] == pair[1]["score"] {
                assert!(pair[0]["path"].as_str() < pair[1]["path"].as_str());
            }
        }
    }
    // Every limit gives a prefix of the longest list, ties included, and
    // a limit equal to the count is not truncated.
    for kind in ["--symbols", "--files"] {
        let all = world
            .run(BINS[0], &["rank", kind, "--limit", "100"])
            .envelope();
        let all = all["data"]["hits"].as_array().expect("hits").clone();
        for limit in 1..=all.len() {
            let text = limit.to_string();
            let env = world
                .run(BINS[0], &["rank", kind, "--limit", &text])
                .envelope();
            assert_eq!(
                env["data"]["hits"].as_array().expect("hits")[..],
                all[..limit]
            );
            assert_eq!(
                env["data"]["truncated"],
                limit < all.len(),
                "{kind} {limit}"
            );
        }
    }
    let one = world
        .run(BINS[0], &["rank", "--symbols", "--limit", "1"])
        .envelope();
    assert_eq!(one["data"]["hits"].as_array().expect("hits").len(), 1);
    assert_eq!(one["data"]["truncated"], true);
    let top = &one["data"]["hits"][0];
    let default = world.run(BINS[0], &["rank"]).envelope();
    assert_eq!(&default["data"]["hits"][0], top);
}

#[test]
fn a_focus_personalises_the_rank_and_leads_the_map() {
    let world = World::new("graph");
    let store = world.store();
    let uses = file_id(&store, "app/src/uses.rs");
    let lib = file_id(&store, "kit/src/lib.rs");
    let computed = personalised(&store, &[uses, lib]).expect("personalised");
    let args = [
        "rank",
        "--files",
        "--focus",
        "./kit/src/lib.rs",
        "--focus",
        "app/src/uses.rs",
        "--focus",
        "kit/src/lib.rs",
    ];
    let run = world.run(BINS[0], &args);
    assert_eq!(
        run.envelope()["data"]["focus"],
        serde_json::json!(["app/src/uses.rs", "kit/src/lib.rs"])
    );
    assert_eq!(
        printed_scores(&run.stdout),
        top(&computed.files, usize::MAX)
    );
    let run = world.run(
        BINS[1],
        &[
            "rank",
            "--focus",
            "kit/src/lib.rs",
            "--focus",
            "app/src/uses.rs",
        ],
    );
    assert_eq!(printed_scores(&run.stdout), top(&computed.symbols, 20));

    let env = world
        .run(
            BINS[0],
            &["map", "--budget", "500", "--focus", "app/src/uses.rs"],
        )
        .envelope();
    let text = env["data"]["text"].as_str().expect("text");
    let first_file = text.lines().nth(1).expect("a file line");
    assert!(first_file.starts_with("app/src/uses.rs  [r "), "{text}");
}

#[test]
fn map_data_is_the_rendered_map() {
    for fixture in ["graph", "impact"] {
        let world = World::new(fixture);
        let store = world.store();
        for budget in [60_u64, 150, 500, 4000, 100_000] {
            for (name, detail) in [
                ("files", Detail::Files),
                ("symbols", Detail::Symbols),
                ("full", Detail::Full),
            ] {
                let want = render(
                    &store,
                    &MapRequest {
                        budget,
                        focus_files: Vec::new(),
                        detail,
                    },
                )
                .expect("render");
                let budget_text = budget.to_string();
                let run = world.run(
                    BINS[0],
                    &["map", "--budget", &budget_text, "--detail", name],
                );
                assert_eq!(
                    raw_keys(&run.stdout, value_at(&run.stdout, "data", 0)),
                    [
                        "text",
                        "used_tokens",
                        "budget",
                        "files_shown",
                        "files_omitted",
                        "symbols_shown",
                        "symbols_omitted",
                        "rank_epoch",
                    ]
                );
                let env = run.envelope();
                let data = &env["data"];
                assert_eq!(data["text"], want.text);
                assert_eq!(data["used_tokens"], want.used_tokens);
                assert!(want.used_tokens <= budget);
                assert_eq!(data["budget"], budget);
                assert_eq!(data["files_shown"], want.files_shown);
                assert_eq!(data["files_omitted"], want.files_omitted);
                assert_eq!(data["symbols_shown"], want.symbols_shown);
                assert_eq!(data["symbols_omitted"], want.symbols_omitted);
                assert_eq!(data["rank_epoch"], want.rank_epoch);
            }
        }
        let symbols = world.run(BINS[1], &["map", "--budget", "500"]);
        let explicit = world.run(BINS[1], &["map", "--budget", "500", "--detail", "symbols"]);
        assert_eq!(symbols.stdout, explicit.stdout, "symbols is the default");
    }
}

#[test]
fn exits_are_usage_no_index_and_not_found() {
    let world = World::unindexed("graph");
    for bin in BINS {
        for args in [&["rank"][..], &["map", "--budget", "500"]] {
            let run = world.run(bin, args);
            assert_eq!(run.code, Some(2), "{args:?}");
            assert_eq!(run.envelope()["error"]["kind"], "no_index");
        }
    }
    let world = World::new("graph");
    for bin in BINS {
        for args in [
            &["rank", "--focus", "nope.rs"][..],
            &["rank", "--focus", "app/src/uses.rs", "--focus", ""],
            &["map", "--budget", "500", "--focus", "app/src"],
            &["rank", "--repo", "missing"],
            &["map", "--budget", "500", "--repo", "missing"],
        ] {
            let run = world.run(bin, args);
            assert_eq!(run.code, Some(3), "{args:?}");
            assert_eq!(run.envelope()["error"]["kind"], "not_found");
        }
        let absolute = world.tree.path().join("app/src/uses.rs");
        let absolute = absolute.to_str().expect("utf-8 path");
        let run = world.run(bin, &["rank", "--focus", absolute]);
        assert_eq!(run.code, Some(3));
        assert!(!run.stdout.contains(absolute), "{}", run.stdout);

        let run = world.run(bin, &["map", "--budget", "1"]);
        assert_eq!(run.code, Some(1));
        let error = &run.envelope()["error"];
        assert_eq!(error["kind"], "usage");
        let message = error["message"].as_str().expect("message");
        assert!(
            message.starts_with("budget_too_small: budget 1 is too small"),
            "{message}"
        );

        for args in [
            &["rank", "--files", "--symbols"][..],
            &["rank", "--limit", "0"],
            &["rank", "--limit", "101"],
            &["rank", "--limit", "many"],
            &["rank", "app"],
            &["map"],
            &["map", "--budget", "0"],
            &["map", "--budget", "100001"],
            &["map", "--budget", "500", "--detail", "all"],
            &["map", "--budget", "500", "extra"],
        ] {
            let run = world.run(bin, args);
            assert_eq!(run.code, Some(1), "{args:?}");
            assert!(run.stdout.is_empty(), "{args:?}");
            assert!(!run.stderr.is_empty(), "{args:?}");
        }
        for args in [
            &["rank", "--limit", "100"][..],
            &["map", "--budget", "100000"],
        ] {
            assert_eq!(world.run(bin, args).code, Some(0), "{args:?}");
        }
    }
}

#[test]
fn reads_write_nothing_repeat_byte_for_byte_and_hold_no_absolute_path() {
    let world = World::new("graph");
    let reads: [&[&str]; 6] = [
        &["rank"],
        &["rank", "--files", "--limit", "100"],
        &["rank", "--focus", "app/src/uses.rs"],
        &["map", "--budget", "500"],
        &[
            "map",
            "--budget",
            "150",
            "--detail",
            "full",
            "--focus",
            "kit/src/lib.rs",
        ],
        &["map", "--budget", "1"],
    ];
    let first: Vec<String> = reads
        .iter()
        .map(|args| world.run(BINS[0], args).stdout)
        .collect();
    let root = world.tree.path().to_str().expect("utf-8 path").to_owned();
    let data = world.data.path().to_str().expect("utf-8 path").to_owned();
    for out in &first {
        assert!(!out.contains(&root) && !out.contains(&data), "{out}");
        assert!(!out.contains("\"path\":\"/"), "{out}");
    }

    // With the stored rank current, and then stale, every read gives the
    // same bytes apart from `meta`, whose `rank_epoch` is the stored rank's,
    // and leaves the database as it was.
    let epoch = world.store().graph_epoch().expect("graph epoch");
    for stale in [false, true] {
        if stale {
            world
                .store()
                .write_rank(epoch - 1, &[(1, 1.0)], &[])
                .expect("write a stale rank");
        }
        let before = world.bytes();
        for bin in BINS {
            for (args, want) in reads.iter().zip(&first) {
                let out = world.run(bin, args).stdout;
                assert_eq!(strip_meta(&out), strip_meta(want), "{args:?} stale {stale}");
                let rank_epoch = if stale { epoch - 1 } else { epoch };
                assert!(
                    out.starts_with("{\"ok\":false,")
                        || out.contains(&format!("\"rank_epoch\":{rank_epoch}}},\"error\"")),
                    "{args:?} stale {stale}: {out}"
                );
            }
        }
        assert_eq!(world.bytes(), before, "stale {stale}");
    }
}

/// `line` without its top-level `meta`, which holds no nested object.
fn strip_meta(line: &str) -> String {
    match line.find(",\"meta\":{") {
        Some(start) => {
            let end = start + line[start..].find('}').expect("meta closes") + 1;
            format!("{}{}", &line[..start], &line[end..])
        }
        None => line.to_owned(),
    }
}

#[test]
fn equal_scores_go_by_path_then_line() {
    // Two files alike, no edges and no boosted item: every symbol and both
    // files tie. Methods get their ids after the free functions, so id
    // order is not line order.
    let file = "struct S;\nimpl S {\n    fn m1(&self) {}\n    fn m2(&self) {}\n}\n\
                fn f1() {}\nfn f2() {}\n";
    let world = World::generated(&[
        ("src/lib.rs", "mod a;\nmod b;\n"),
        ("src/a.rs", file),
        ("src/b.rs", file),
    ]);
    let env = world.run(BINS[0], &["rank", "--files"]).envelope();
    let hits = env["data"]["hits"].as_array().expect("hits");
    let paths: Vec<&str> = hits
        .iter()
        .map(|hit| hit["path"].as_str().expect("path"))
        .collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs"]);
    assert_eq!(hits[0]["score"], hits[1]["score"]);

    let all = world.run(BINS[0], &["rank", "--limit", "100"]).envelope();
    let all = all["data"]["hits"].as_array().expect("hits").clone();
    let order: Vec<(String, i64)> = all
        .iter()
        .map(|hit| {
            assert_eq!(hit["score"], all[0]["score"], "every symbol ties");
            (
                hit["path"].as_str().expect("path").to_owned(),
                hit["line"].as_i64().expect("line"),
            )
        })
        .collect();
    let mut want = order.clone();
    want.sort();
    assert_eq!(order, want);
    assert_eq!(order.len(), 10);
    for limit in 1..=all.len() {
        let text = limit.to_string();
        let env = world.run(BINS[1], &["rank", "--limit", &text]).envelope();
        assert_eq!(
            env["data"]["hits"].as_array().expect("hits")[..],
            all[..limit]
        );
    }
}
