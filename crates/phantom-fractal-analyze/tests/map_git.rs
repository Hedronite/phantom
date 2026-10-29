//! The map with a co-change scan, on a synthetic git fixture
//! (SPEC-w8 G2).
//!
//! The fixture is a small crate whose history is written by the `git`
//! CLI in a temp dir (`git init`, one `git fast-import` stream with fixed
//! times and identity, `git reset --hard`), with no system or global
//! config, a temp `HOME` and no hooks. It is indexed, mined with
//! `phantom-fractal-git`, and the scan is written to the store. Every
//! commit is at one time, so every decay is 1. With `N` = 200 commits:
//!
//! - `hub.rs` changes with each of `p1.rs` to `p4.rs` three times: lift
//!   3 * 200 / (12 * 3) = 16.7, so `hub.rs` names the first three by path
//!   and each `pN.rs` names `hub.rs`;
//! - `e.rs` and `f.rs` change together three times: lift 66.7;
//! - `c.rs` and `d.rs` change together three times, and `c.rs` 77 more
//!   times alone: lift 3 * 200 / (80 * 3) = 2.5, just above the cut;
//! - `a.rs` and `b.rs` change together three times, and `a.rs` 97 more
//!   times alone: lift 3 * 200 / (100 * 3) = 2, which is not above 2, so
//!   neither names the other.
//!
//! Goldens are `tests/goldens/map/cochange-<detail>-<budget>.txt`;
//! `PHANTOM_BLESS=1` rewrites them.

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{Detail, MapRequest, estimate_tokens, packing_budget, render};
use phantom_fractal_extract::index;
use phantom_fractal_git::{Window, mine};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const GOLDENS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens/map");
const BUDGETS: [u64; 3] = [80, 200, 600];
const DETAILS: [(Detail, &str); 3] = [
    (Detail::Files, "files"),
    (Detail::Symbols, "symbols"),
    (Detail::Full, "full"),
];
/// Every commit's time, unix seconds (2025-06-15).
const TIME: i64 = 1_750_000_000;
const FILES: [&str; 12] = [
    "hub", "p1", "p2", "p3", "p4", "e", "f", "c", "d", "a", "b", "noise",
];

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-analyze-map-git-{tag}-{}-{nanos}",
            process::id()
        ));
        fs::create_dir_all(path.join("repo")).expect("temp dir");
        fs::create_dir_all(path.join("home")).expect("temp home");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `git` in `dir` with the fixed, isolated environment.
fn git(dir: &Path, home: &Path, args: &[&str], stdin: Option<&[u8]>) {
    let mut child = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("LC_ALL", "C")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run git");
    if let Some(bytes) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(bytes)
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("git output");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The fixture's fast-import stream: one commit per entry of `commits`,
/// each writing the named files with a new revision line.
fn stream() -> String {
    let mut commits: Vec<Vec<&str>> = vec![vec!["lib"]];
    for partner in ["p1", "p2", "p3", "p4"] {
        commits.extend((0..3).map(|_| vec!["hub", partner]));
    }
    commits.extend((0..3).map(|_| vec!["e", "f"]));
    commits.extend((0..3).map(|_| vec!["c", "d"]));
    commits.extend((0..3).map(|_| vec!["a", "b"]));
    commits.extend((0..4).map(|_| vec!["noise"]));
    commits.extend((0..77).map(|_| vec!["c"]));
    commits.extend((0..97).map(|_| vec!["a"]));
    assert_eq!(commits.len(), 200);

    let mut text = String::new();
    for (revision, files) in commits.iter().enumerate() {
        let _ = writeln!(text, "commit refs/heads/main");
        let _ = writeln!(text, "committer Test <test@example.invalid> {TIME} +0000");
        let _ = writeln!(text, "data 7\nmessage");
        for stem in files {
            let (path, body) = if *stem == "lib" {
                let mods: String = FILES.iter().map(|m| format!("pub mod {m};\n")).collect();
                let manifest =
                    "[package]\nname = \"cofix\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
                let _ = writeln!(text, "M 100644 inline Cargo.toml");
                let _ = writeln!(text, "data {}\n{manifest}", manifest.len());
                ("src/lib.rs".to_owned(), mods)
            } else {
                (
                    format!("src/{stem}.rs"),
                    format!(
                        "/// The {stem} module's entry.\npub fn {stem}_entry() -> u32 {{\n    {revision}\n}}\n"
                    ),
                )
            };
            let _ = writeln!(text, "M 100644 inline {path}");
            let _ = writeln!(text, "data {}\n{body}", body.len());
        }
    }
    text
}

/// The fixture built, indexed and, when `scan` is true, mined into the
/// store. Returns the store and the full HEAD oid.
fn fixture(tag: &str, scan: bool) -> (TempDir, Store, String) {
    let dir = TempDir::new(tag);
    let (root, home) = (dir.0.join("repo"), dir.0.join("home"));
    git(&root, &home, &["init", "-q", "-b", "main"], None);
    git(
        &root,
        &home,
        &["fast-import", "--quiet"],
        Some(stream().as_bytes()),
    );
    git(&root, &home, &["reset", "-q", "--hard", "main"], None);
    let mut store = Store::open(&dir.0.join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    index(&root, &mut store, &rust).expect("index");
    let paths = store
        .files()
        .expect("files")
        .into_iter()
        .map(|file| file.path)
        .collect();
    let scan_found = mine(&root, &Window::default(), &paths)
        .expect("mine")
        .expect("a git root");
    let head = scan_found.last_oid.clone().expect("head");
    if scan {
        store.write_cochange(&scan_found).expect("write scan");
    }
    (dir, store, head)
}

fn request(budget: u64, detail: Detail) -> MapRequest {
    MapRequest {
        budget,
        focus_files: Vec::new(),
        detail,
    }
}

/// Compares `text` with golden `name`, or writes it under `PHANTOM_BLESS`.
fn assert_golden(name: &str, text: &str) {
    let path = Path::new(GOLDENS).join(format!("{name}.txt"));
    if std::env::var_os("PHANTOM_BLESS").is_some() {
        fs::write(&path, text).expect("write golden");
        return;
    }
    let want = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert_eq!(text, want, "golden {name}");
}

#[test]
fn cochange_goldens_at_three_budgets_and_every_detail() {
    let (_dir, store, head) = fixture("goldens", true);
    for budget in BUDGETS {
        for (detail, label) in DETAILS {
            let map = render(&store, &request(budget, detail)).expect("map");
            assert_eq!(map.used_tokens, estimate_tokens(&map.text));
            assert!(map.used_tokens <= packing_budget(budget));
            assert!(map.text.is_ascii());
            let header = map.text.lines().next().expect("header");
            assert!(
                header.starts_with(&format!(
                    "phantom-fractal map | head {} | budget {budget} |",
                    &head[..7]
                )),
                "{header}"
            );
            assert_golden(&format!("cochange-{label}-{budget}"), &map.text);
        }
    }
}

#[test]
fn also_changes_names_up_to_three_partners_above_lift_two() {
    let (_dir, store, _) = fixture("partners", true);
    let map = render(&store, &request(100_000, Detail::Files)).expect("map");
    let line = |path: &str| {
        map.text
            .lines()
            .find(|line| line.starts_with(&format!("{path}  [r ")))
            .unwrap_or_else(|| panic!("no line for {path}:\n{}", map.text))
            .to_owned()
    };
    assert!(line("src/hub.rs").ends_with("]  also-changes: p1.rs, p2.rs, p3.rs"));
    for partner in ["p1", "p2", "p3", "p4"] {
        assert!(line(&format!("src/{partner}.rs")).ends_with("]  also-changes: hub.rs"));
    }
    assert!(line("src/e.rs").ends_with("]  also-changes: f.rs"));
    assert!(line("src/f.rs").ends_with("]  also-changes: e.rs"));
    // Lift 2.5 is above 2.
    assert!(line("src/c.rs").ends_with("]  also-changes: d.rs"));
    assert!(line("src/d.rs").ends_with("]  also-changes: c.rs"));
    // Lift exactly 2 is not above 2.
    for path in ["src/a.rs", "src/b.rs", "src/noise.rs", "src/lib.rs"] {
        assert!(line(path).ends_with(']'), "{}", line(path));
    }
}

#[test]
fn without_a_scan_the_map_has_no_head_and_no_partners() {
    let (_dir, store, _) = fixture("noscan", false);
    for (detail, _) in DETAILS {
        let map = render(&store, &request(600, detail)).expect("map");
        assert!(
            map.text
                .starts_with("phantom-fractal map | budget 600 | used ")
        );
        assert!(!map.text.contains("also-changes"));
        assert!(!map.text.contains("| head "));
    }
}
