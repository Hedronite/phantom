//! The timing harness (SPEC-w13 G0c).
//!
//! [`timings_of_the_named_tree`] is ignored. It reads the tree named by
//! `PHANTOM_TIMING_ROOT`, spawns the built `fract`, and prints one JSON
//! line holding the median of five runs of each measure, in milliseconds:
//!
//! - `index_cold`: `index` of the root, each run into a fresh data
//!   directory;
//! - `index_edit`: `index` after a one-line edit of one file, made in a
//!   copy of the tree under the test's temp dir, never in the root;
//! - `find`, `callers`, `impact` and `map` (`--budget 2000`) against the
//!   root's index, on names the rank fixes.
//!
//! The names follow from the index alone. The edited file is the
//! best-ranked one. `find` takes the bare name of the best-ranked symbol it
//! lists (it lists items, not members). `callers` and `impact` take the
//! qualified name of the best-ranked symbol whose `callers` reply has a
//! hit.
//!
//! Nothing is written under the root. The edit happens in the copy, every
//! data directory sits in the temp dir, and the root's entries (path,
//! kind, length, modification time) must be the same after the run as
//! before it. The JSON names no path.
//!
//! [`timings_run_once_on_the_fixtures`] runs the same harness once on
//! `fixtures/` in CI, and asserts no time.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Instant, SystemTime};

use serde_json::{Map, Value, json};

const FRACT: &str = env!("CARGO_BIN_EXE_fract");
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
/// Runs per measure in the ignored test. Each measure is their median.
const RUNS: usize = 5;
/// The budget `map` is timed at.
const MAP_BUDGET: &str = "2000";
/// How far down the rank the names are looked for.
const RANK_DEPTH: &str = "100";
/// The measures, in the order the JSON lists them.
const MEASURES: [&str; 6] = [
    "index_cold",
    "index_edit",
    "find",
    "callers",
    "impact",
    "map",
];

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-timings-{label}-{}-{}",
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

    /// A new, empty directory `name` inside this one.
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).expect("create dir");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One `fract` run: its exit code, its envelope, and its wall time.
struct Run {
    code: Option<i32>,
    envelope: Value,
    millis: f64,
}

impl Run {
    fn data(&self) -> &Value {
        &self.envelope["data"]
    }

    /// True when the run exited 0 with `ok` and its `data.hits` is not
    /// empty.
    fn has_hits(&self) -> bool {
        self.code == Some(0)
            && self.envelope["ok"] == true
            && self.data()["hits"]
                .as_array()
                .is_some_and(|hits| !hits.is_empty())
    }
}

/// Runs `fract` with `args` and the data directory `data`, from `cwd`, and
/// times it from spawn to exit.
fn fract(data: &Path, cwd: &Path, args: &[&str]) -> Run {
    let start = Instant::now();
    let output = Command::new(FRACT)
        .args(args)
        .current_dir(cwd)
        .env("PHANTOM_FRACTAL_DATA", data)
        .output()
        .expect("run fract");
    let millis = start.elapsed().as_secs_f64() * 1e3;
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let line = stdout.lines().next().unwrap_or_default();
    let envelope = serde_json::from_str(line).unwrap_or_else(|err| {
        panic!(
            "fract {args:?}: {err}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    Run {
        code: output.status.code(),
        envelope,
        millis,
    }
}

/// As [`fract`], and the run must exit 0 with `ok`.
fn ok(data: &Path, cwd: &Path, args: &[&str]) -> Run {
    let run = fract(data, cwd, args);
    assert!(
        run.code == Some(0) && run.envelope["ok"] == true,
        "fract {args:?} exited {:?}: {}",
        run.code,
        run.envelope
    );
    run
}

/// The median of `samples`, rounded to a tenth of a millisecond.
fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    let value = if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    };
    tenths(value)
}

fn tenths(millis: f64) -> f64 {
    (millis * 10.0).round() / 10.0
}

/// Every entry under `root`, links not followed: its path relative to the
/// root, whether it is a directory, its length and its modification time.
/// Two equal lists mean nothing under the root was written.
fn entries(root: &Path) -> Vec<(PathBuf, bool, u64, Option<SystemTime>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path).expect("metadata");
        if meta.is_dir() {
            for entry in fs::read_dir(&path).expect("read dir") {
                stack.push(entry.expect("entry").path());
            }
        }
        out.push((
            path.strip_prefix(root).expect("under root").to_path_buf(),
            meta.is_dir(),
            meta.len(),
            meta.modified().ok(),
        ));
    }
    out.sort();
    out
}

/// Copies the tree `from` to `to` as the index walk sees it, and returns
/// how many symlinks it left out. A `target` directory the walk prunes (at
/// the top, or beside a `Cargo.toml`) is left out, and so is every entry
/// that is neither a directory nor a plain file. A `.git` directory is
/// copied, so the copy keeps its history, but never its `index` file,
/// which is not read.
fn copy_tree(from: &Path, to: &Path, top: bool) -> usize {
    fs::create_dir_all(to).expect("create copy dir");
    let git = from.file_name().is_some_and(|name| name == ".git");
    let mut links = 0;
    for entry in fs::read_dir(from).expect("read tree dir") {
        let entry = entry.expect("tree entry");
        let kind = entry.file_type().expect("file type");
        let name = entry.file_name();
        if kind.is_symlink() {
            links += 1;
        } else if kind.is_dir() {
            let pruned = name == "target" && (top || from.join("Cargo.toml").is_file());
            if !pruned {
                links += copy_tree(&entry.path(), &to.join(&name), false);
            }
        } else if kind.is_file() && !(git && name == "index") {
            fs::copy(entry.path(), to.join(&name)).expect("copy file");
        }
    }
    links
}

/// Panics if any string or key in `value` starts with `/` or holds one of
/// `paths`.
fn assert_path_free(value: &Value, paths: &[String]) {
    let check = |text: &str| {
        assert!(
            !text.starts_with('/') && paths.iter().all(|path| !text.contains(path.as_str())),
            "a path in the timing JSON: {text}"
        );
    };
    match value {
        Value::String(text) => check(text),
        Value::Array(items) => items.iter().for_each(|item| assert_path_free(item, paths)),
        Value::Object(fields) => {
            for (key, item) in fields {
                check(key);
                assert_path_free(item, paths);
            }
        }
        _ => {}
    }
}

/// The best-ranked symbol whose `probe` run has a hit: its 1-based rank
/// and the name `pick` takes from its rank hit.
fn first_with_hits(
    hits: &[Value],
    pick: &str,
    mut probe: impl FnMut(&str) -> Run,
) -> (usize, String) {
    hits.iter()
        .enumerate()
        .find_map(|(at, hit)| {
            let name = hit[pick].as_str()?;
            probe(name).has_hits().then(|| (at + 1, name.to_owned()))
        })
        .unwrap_or_else(|| panic!("no ranked symbol in the top {RANK_DEPTH} has a hit by {pick}"))
}

/// Times every measure on `root`, `runs` runs each, and returns the JSON.
fn measure(root: &Path, runs: usize) -> Value {
    let temp = TempDir::new("harness");
    let cwd = temp.path();
    let root_arg = root.to_str().expect("utf-8 root");
    let before = entries(root);
    let mut samples: Map<String, Value> = Map::new();

    let mut cold = Vec::new();
    let mut tree = Value::Null;
    let mut data = PathBuf::new();
    for at in 0..runs {
        data = temp.dir(&format!("data-cold-{at}"));
        let run = ok(&data, cwd, &["index", root_arg]);
        tree = run.data().clone();
        cold.push(run.millis);
    }

    let ranked = ok(
        &data,
        cwd,
        &["rank", "--limit", RANK_DEPTH, "--repo", root_arg],
    );
    let hits = ranked.data()["hits"].as_array().expect("rank hits").clone();
    let (find_rank, find_name) = first_with_hits(&hits, "name", |name| {
        fract(&data, cwd, &["find", name, "--repo", root_arg])
    });
    let (target_rank, target) = first_with_hits(&hits, "qual_name", |name| {
        fract(&data, cwd, &["callers", name, "--repo", root_arg])
    });
    let files = ok(
        &data,
        cwd,
        &["rank", "--files", "--limit", "1", "--repo", root_arg],
    );
    let file = files.data()["hits"][0]["path"]
        .as_str()
        .expect("a best-ranked file")
        .to_owned();

    let copy = cwd.join("copy");
    let links = copy_tree(root, &copy, true);
    let copy_arg = copy.to_str().expect("utf-8 copy");
    let copy_data = temp.dir("data-copy");
    ok(&copy_data, cwd, &["index", copy_arg]);
    let edited = copy.join(&file);
    let mut base = fs::read_to_string(&edited).expect("read the best-ranked file");
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }
    let mut edit = Vec::new();
    for at in 0..runs {
        // Each line is one byte longer than the last, so the file's size
        // alone shows the change.
        let line = format!("fn phantom_timing_edit{}() {{}}\n", "_".repeat(at + 1));
        fs::write(&edited, format!("{base}{line}")).expect("edit the copy");
        let run = ok(&copy_data, cwd, &["index", copy_arg]);
        assert_eq!(
            run.data()["written"],
            1,
            "one file re-indexed: {}",
            run.envelope
        );
        edit.push(run.millis);
    }

    samples.insert("index_cold".into(), json!(cold));
    samples.insert("index_edit".into(), json!(edit));
    let queries: [(&str, Vec<&str>); 4] = [
        ("find", vec!["find", &find_name, "--repo", root_arg]),
        ("callers", vec!["callers", &target, "--repo", root_arg]),
        ("impact", vec!["impact", &target, "--repo", root_arg]),
        (
            "map",
            vec!["map", "--budget", MAP_BUDGET, "--repo", root_arg],
        ),
    ];
    for (measure, args) in &queries {
        let times: Vec<f64> = (0..runs).map(|_| ok(&data, cwd, args).millis).collect();
        samples.insert((*measure).into(), json!(times));
    }

    let version = ok(&data, cwd, &["version"]).data().clone();
    assert_eq!(entries(root), before, "nothing is written under the root");

    let mut medians = Map::new();
    let mut runs_ms = Map::new();
    for key in MEASURES {
        let times: Vec<f64> = samples[key]
            .as_array()
            .expect("samples")
            .iter()
            .map(|time| time.as_f64().expect("a time"))
            .collect();
        medians.insert(key.into(), json!(median(&times)));
        runs_ms.insert(
            key.into(),
            json!(times.iter().copied().map(tenths).collect::<Vec<_>>()),
        );
    }
    let profile = Path::new(FRACT)
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let out = json!({
        "harness": "phantom-timing",
        "schema": 1,
        "fract": version,
        "profile": profile,
        "runs": runs,
        "map_budget": MAP_BUDGET.parse::<u64>().expect("budget"),
        "names": {
            "find": { "name": find_name, "rank": find_rank },
            "callers_impact": { "name": target, "rank": target_rank },
        },
        "edit": {
            "file_rank": 1,
            "change": "one fn line appended to the best-ranked file, in a copy",
            "copy_left_out_symlinks": links,
        },
        "tree": tree,
        "median_ms": medians,
        "runs_ms": runs_ms,
    });
    let mut paths = vec![
        root_arg.to_owned(),
        cwd.to_string_lossy().into_owned(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ];
    if let Some(home) = std::env::var_os("HOME").filter(|home| home.len() > 1) {
        paths.push(home.to_string_lossy().into_owned());
    }
    assert_path_free(&out, &paths);
    out
}

#[test]
#[ignore = "times a whole tree; set PHANTOM_TIMING_ROOT and run with --run-ignored"]
fn timings_of_the_named_tree() {
    let root = std::env::var_os("PHANTOM_TIMING_ROOT").expect("PHANTOM_TIMING_ROOT names a tree");
    let root = Path::new(&root)
        .canonicalize()
        .expect("the timing root exists");
    println!("{}", measure(&root, RUNS));
}

#[test]
fn timings_run_once_on_the_fixtures() {
    let root = Path::new(FIXTURES).canonicalize().expect("fixtures");
    let out = measure(&root, 1);
    assert_eq!(out["harness"], "phantom-timing");
    assert_eq!(out["runs"], 1);
    assert_eq!(out["map_budget"], 2000);
    for key in MEASURES {
        assert!(
            out["median_ms"][key].as_f64().is_some_and(|ms| ms >= 0.0),
            "{key}: {out}"
        );
        assert_eq!(
            out["runs_ms"][key].as_array().map(Vec::len),
            Some(1),
            "{key}"
        );
    }
    assert!(
        out["names"]["find"]["rank"]
            .as_u64()
            .is_some_and(|rank| rank >= 1)
    );
    assert!(
        out["names"]["callers_impact"]["name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    assert!(
        out["tree"]["written"]
            .as_u64()
            .is_some_and(|files| files > 0)
    );
}

#[test]
fn the_copy_leaves_out_what_the_walk_prunes_and_the_git_index() {
    let temp = TempDir::new("copy");
    let root = temp.dir("root");
    for (path, body) in [
        ("Cargo.toml", "[package]\n"),
        ("src/lib.rs", "pub fn f() {}\n"),
        ("src/target/kept.rs", "pub fn g() {}\n"),
        ("target/debug/out", "x"),
        ("sub/Cargo.toml", "[package]\n"),
        ("sub/target/out", "x"),
        ("sub/deep/target/kept", "x"),
        (".git/HEAD", "ref: refs/heads/main\n"),
        (".git/index", "DIRC"),
        (".git/refs/index", "kept"),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, body).expect("write");
    }
    let copy = temp.path().join("copy");
    assert_eq!(copy_tree(&root, &copy, true), 0);
    let mut files: Vec<String> = entries(&copy)
        .into_iter()
        .filter(|(_, dir, _, _)| !dir)
        .map(|(path, _, _, _)| path.to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            ".git/HEAD",
            ".git/refs/index",
            "Cargo.toml",
            "src/lib.rs",
            "src/target/kept.rs",
            "sub/Cargo.toml",
            "sub/deep/target/kept",
        ]
    );
}

#[test]
fn the_median_is_the_middle_sample() {
    assert_eq!(median(&[30.0, 10.0, 20.0]), 20.0);
    assert_eq!(median(&[4.04]), 4.0);
    assert_eq!(median(&[1.0, 2.0, 4.0, 9.0]), 3.0);
    assert_eq!(median(&[5.0, 1.0, 3.0, 2.0, 4.0]), 3.0);
}
