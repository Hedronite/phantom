//! Synthetic git roots for the co-change tests (SPEC-w8 G3), and the
//! fixture copies and raw-line helpers of the freshness tests (SPEC-w9
//! G0c).
//!
//! Each repository lives in a fresh temp dir and is built by the `git`
//! CLI, run only here: `git init`, then one `git fast-import` stream with
//! fixed committer times and identity, then `git reset --hard` to check
//! the tree out. Every run clears the environment and sets
//! `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, a temp `HOME`
//! and `core.hooksPath=/dev/null`, so no hook or outside config runs.
//! Phantom itself never runs `git`.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

/// HEAD's committer time, unix seconds (2025-06-15).
pub const HEAD: i64 = 1_750_000_000;
pub const DAY: i64 = 86_400;

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-cochange-{label}-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path.canonicalize().expect("canonical temp dir"))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `git fast-import` stream of commits on `main`, each a set of whole
/// files written at a time.
#[derive(Default)]
pub struct Script {
    text: String,
    marks: u32,
    /// The first commit goes on top of the `main` already imported.
    continues: bool,
}

impl Script {
    /// A stream whose first commit goes on top of the existing `main`.
    pub fn after_main() -> Self {
        Self {
            continues: true,
            ..Self::default()
        }
    }

    pub fn commit(&mut self, time: i64, files: &[(&str, String)]) {
        self.marks += 1;
        let text = &mut self.text;
        let _ = writeln!(text, "commit refs/heads/main");
        let _ = writeln!(text, "mark :{}", self.marks);
        let _ = writeln!(text, "committer Test <test@example.invalid> {time} +0000");
        let _ = writeln!(text, "data 7\nmessage");
        if self.continues && self.marks == 1 {
            let _ = writeln!(text, "from refs/heads/main^0");
        }
        for (path, body) in files {
            let _ = writeln!(text, "M 100644 inline {path}");
            let _ = writeln!(text, "data {}\n{body}", body.len());
        }
    }
}

/// A synthetic repository whose work tree top is `root`.
pub struct Repo {
    dir: TempDir,
    pub root: PathBuf,
    home: PathBuf,
}

impl Repo {
    pub fn new(label: &str, script: &Script) -> Self {
        let dir = TempDir::new(label);
        let root = dir.path().join("repo");
        let home = dir.path().join("home");
        fs::create_dir_all(&root).expect("repo dir");
        fs::create_dir_all(&home).expect("home dir");
        let repo = Self { dir, root, home };
        repo.git(&["init", "-q", "-b", "main"], None);
        repo.import(script);
        repo
    }

    /// Imports more commits onto `main` and checks the tree out again.
    pub fn import(&self, script: &Script) {
        self.git(&["fast-import", "--quiet"], Some(script.text.as_bytes()));
        self.git(&["reset", "-q", "--hard", "main"], None);
    }

    /// Runs `git` in the root with the fixed, isolated environment.
    pub fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> String {
        let mut child = Command::new("git")
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
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
            .stdout(Stdio::piped())
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
        String::from_utf8(output.stdout).expect("utf-8 output")
    }

    /// The full hex id of `rev`.
    pub fn rev(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev], None).trim().to_owned()
    }
}

/// The files of the package every history below changes.
pub const FILES: [&str; 7] = ["a", "b", "c", "d", "e", "f", "g"];

/// `src/<name>.rs` with one function and a revision comment.
pub fn module(name: &str, revision: u32) -> (String, String) {
    (
        format!("src/{name}.rs"),
        format!("/// {name}'s function.\npub fn f{name}() {{}}\n// revision {revision}\n"),
    )
}

/// A package whose history pairs `a` with `b` five times, with `c` four
/// times and with `g` three times, pairs `d` with `e` four times, and
/// changes `f` alone six times, one commit a day back from [`HEAD`]. The
/// first commit, 60 days back, adds every file.
pub fn history() -> Script {
    let mut script = Script::default();
    let mut lib = String::new();
    for name in FILES {
        lib.push_str(&format!("pub mod {name};\n"));
    }
    lib.push_str("\npub fn top() {\n    a::fa();\n    b::fb();\n}\n");
    let mut first = vec![
        (
            "Cargo.toml".to_owned(),
            "[package]\nname = \"history\"\nversion = \"0.0.0\"\nedition = \"2024\"\n".to_owned(),
        ),
        ("src/lib.rs".to_owned(), lib),
    ];
    first.extend(FILES.iter().map(|name| module(name, 0)));
    let borrowed: Vec<(&str, String)> = first
        .iter()
        .map(|(path, body)| (path.as_str(), body.clone()))
        .collect();
    script.commit(HEAD - 60 * DAY, &borrowed);
    let mut changes: Vec<Vec<&str>> = Vec::new();
    changes.extend(std::iter::repeat_n(vec!["a", "b"], 5));
    changes.extend(std::iter::repeat_n(vec!["a", "c"], 4));
    changes.extend(std::iter::repeat_n(vec!["a", "g"], 3));
    changes.extend(std::iter::repeat_n(vec!["d", "e"], 4));
    changes.extend(std::iter::repeat_n(vec!["f"], 6));
    let count = changes.len() as i64;
    for (step, names) in (1..).zip(&changes) {
        let files: Vec<(String, String)> = names.iter().map(|name| module(name, step)).collect();
        let borrowed: Vec<(&str, String)> = files
            .iter()
            .map(|(path, body)| (path.as_str(), body.clone()))
            .collect();
        script.commit(HEAD - (count - i64::from(step)) * DAY, &borrowed);
    }
    script
}

/// A package where `hub` changes with each of `s0` to `s7` in 3 commits,
/// and `pad` alone in 40, one commit a day back from [`HEAD`], after a
/// first commit 100 days back: `hub` has 8 partners, each of lift above
/// 1.5.
pub fn star() -> Script {
    let mut script = Script::default();
    let spokes: Vec<String> = (0..8).map(|i| format!("s{i}")).collect();
    let mut names: Vec<&str> = vec!["hub", "pad"];
    names.extend(spokes.iter().map(String::as_str));
    let mut lib: String = names
        .iter()
        .map(|name| format!("pub mod {name};\n"))
        .collect();
    lib.push_str("\npub fn top() {\n    hub::fhub();\n}\n");
    let mut first = vec![
        (
            "Cargo.toml".to_owned(),
            "[package]\nname = \"star\"\nversion = \"0.0.0\"\nedition = \"2024\"\n".to_owned(),
        ),
        ("src/lib.rs".to_owned(), lib),
    ];
    first.extend(names.iter().map(|name| module(name, 0)));
    let borrowed: Vec<(&str, String)> = first
        .iter()
        .map(|(path, body)| (path.as_str(), body.clone()))
        .collect();
    script.commit(HEAD - 100 * DAY, &borrowed);
    let mut changes: Vec<Vec<&str>> = Vec::new();
    for spoke in &spokes {
        changes.extend(std::iter::repeat_n(vec!["hub", spoke.as_str()], 3));
    }
    changes.extend(std::iter::repeat_n(vec!["pad"], 40));
    let count = changes.len() as i64;
    for (step, names) in (1..).zip(&changes) {
        let files: Vec<(String, String)> = names.iter().map(|name| module(name, step)).collect();
        let borrowed: Vec<(&str, String)> = files
            .iter()
            .map(|(path, body)| (path.as_str(), body.clone()))
            .collect();
        script.commit(HEAD - (count - i64::from(step)) * DAY, &borrowed);
    }
    script
}

/// Every entry under `root`: its relative path, whether it is a
/// directory, its length and mtime and, for files, its bytes. Two equal
/// snapshots mean nothing was written.
pub fn snapshot(root: &Path) -> Vec<(PathBuf, bool, u64, SystemTime, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let meta = fs::symlink_metadata(&dir).expect("dir metadata");
        out.push((
            dir.strip_prefix(root).expect("under root").to_path_buf(),
            true,
            0,
            meta.modified().expect("mtime"),
            Vec::new(),
        ));
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            let meta = fs::symlink_metadata(&path).expect("metadata");
            if meta.is_dir() {
                stack.push(path);
            } else {
                out.push((
                    path.strip_prefix(root).expect("under root").to_path_buf(),
                    false,
                    meta.len(),
                    meta.modified().expect("mtime"),
                    fs::read(&path).unwrap_or_default(),
                ));
            }
        }
    }
    out.sort();
    out
}

/// The bytes and mtimes of every `graph.db` and `graph.db-wal` under the
/// data directory `data`. The shared-memory file is left out: readers may
/// touch it.
pub fn database(data: &Path) -> Vec<(PathBuf, Vec<u8>, Option<SystemTime>)> {
    snapshot(data)
        .into_iter()
        .filter(|(path, dir, _, _, _)| {
            !dir && (path.ends_with("graph.db") || path.ends_with("graph.db-wal"))
        })
        .map(|(path, _, _, mtime, bytes)| (path, bytes, Some(mtime)))
        .collect()
}

/// A copy of the fixture tree `name` at `to`.
pub fn copy_fixture(name: &str, to: &Path) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    copy_tree(&fixtures.join(name), to);
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

/// The keys of the object that starts at `at` in `raw`, at its own depth,
/// in the order they were printed. A parsed `Value` sorts its keys.
pub fn raw_keys(raw: &str, at: usize) -> Vec<String> {
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
pub fn value_at(raw: &str, key: &str, from: usize) -> usize {
    let needle = format!("\"{key}\":");
    from + raw[from..]
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key}"))
        + needle.len()
}

/// `line` without its top-level `meta`. A `,"meta":{` cannot occur inside
/// a JSON string, and `meta` holds no nested object.
pub fn strip_meta(line: &str) -> String {
    match line.find(",\"meta\":{") {
        Some(start) => {
            let end = start + line[start..].find('}').expect("meta closes") + 1;
            format!("{}{}", &line[..start], &line[end..])
        }
        None => line.to_owned(),
    }
}

/// The path of the one `graph.db` under the data directory `data`.
pub fn database_path(data: &Path) -> PathBuf {
    let repos = data.join("repos");
    let mut found: Vec<PathBuf> = fs::read_dir(&repos)
        .expect("repos dir")
        .map(|entry| entry.expect("entry").path().join("graph.db"))
        .filter(|path| path.exists())
        .collect();
    assert_eq!(found.len(), 1, "one database under {}", repos.display());
    found.remove(0)
}
