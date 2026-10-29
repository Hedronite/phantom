//! Synthetic repositories for the mining tests (SPEC-w8 G1c).
//!
//! Each repository lives in a fresh temp dir and is built by the `git`
//! CLI, run only here: `git init`, then one `git fast-import` stream with
//! fixed committer times and identity, then `git reset --hard` to check
//! the tree out. Every run clears the environment and sets
//! `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, a temp `HOME`
//! and `core.hooksPath=/dev/null`, so no hook or outside config runs.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// HEAD's committer time in most tests, unix seconds (2025-06-15).
pub const HEAD: i64 = 1_750_000_000;
pub const DAY: i64 = 86_400;

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-git-{tag}-{}-{nanos}",
            process::id()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path.canonicalize().expect("canonical temp dir"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One tree operation of a commit.
pub enum Op<'a> {
    Write(&'a str, &'a str),
    Delete(&'a str),
    Rename(&'a str, &'a str),
    /// A symlink at the first path pointing at the second.
    Link(&'a str, &'a str),
}

/// A `git fast-import` stream under construction.
#[derive(Default)]
pub struct Script {
    text: String,
    marks: u32,
}

impl Script {
    /// Commits `ops` on `branch` at `time`. A new branch other than
    /// `main` starts at `from`. Returns the commit's mark.
    pub fn commit(&mut self, branch: &str, time: i64, ops: &[Op]) -> u32 {
        self.commit_with(branch, time, None, None, ops)
    }

    /// Starts `branch` at mark `from`, then commits `ops`.
    pub fn branch(&mut self, branch: &str, from: u32, time: i64, ops: &[Op]) -> u32 {
        self.commit_with(branch, time, Some(from), None, ops)
    }

    /// A merge commit on `branch` with `other` as its second parent.
    pub fn merge(&mut self, branch: &str, other: u32, time: i64, ops: &[Op]) -> u32 {
        self.commit_with(branch, time, None, Some(other), ops)
    }

    fn commit_with(
        &mut self,
        branch: &str,
        time: i64,
        from: Option<u32>,
        merge: Option<u32>,
        ops: &[Op],
    ) -> u32 {
        self.marks += 1;
        let text = &mut self.text;
        let _ = writeln!(text, "commit refs/heads/{branch}");
        let _ = writeln!(text, "mark :{}", self.marks);
        let _ = writeln!(text, "committer Test <test@example.invalid> {time} +0000");
        let _ = writeln!(text, "data 7\nmessage");
        if let Some(from) = from {
            let _ = writeln!(text, "from :{from}");
        }
        if let Some(other) = merge {
            let _ = writeln!(text, "merge :{other}");
        }
        for op in ops {
            match op {
                Op::Write(path, body) => {
                    let _ = writeln!(text, "M 100644 inline {path}");
                    let _ = writeln!(text, "data {}\n{body}", body.len());
                }
                Op::Delete(path) => {
                    let _ = writeln!(text, "D {path}");
                }
                Op::Link(path, target) => {
                    let _ = writeln!(text, "M 120000 inline {path}");
                    let _ = writeln!(text, "data {}\n{target}", target.len());
                }
                Op::Rename(from, to) => {
                    let _ = writeln!(text, "R {from} {to}");
                }
            }
        }
        self.marks
    }
}

/// A synthetic repository: `root` is the work tree top.
pub struct Repo {
    dir: TempDir,
    pub root: PathBuf,
    pub home: PathBuf,
}

impl Repo {
    /// An initialised repository with `script` imported and `main`
    /// checked out. An empty script leaves HEAD unborn.
    pub fn new(tag: &str, script: &Script) -> Self {
        let dir = TempDir::new(tag);
        let root = dir.0.join("repo");
        let home = dir.0.join("home");
        fs::create_dir_all(&root).expect("repo dir");
        fs::create_dir_all(&home).expect("home dir");
        let repo = Self { dir, root, home };
        repo.git(&["init", "-q", "-b", "main"], None);
        if !script.text.is_empty() {
            repo.git(&["fast-import", "--quiet"], Some(script.text.as_bytes()));
            repo.git(&["reset", "-q", "--hard", "main"], None);
        }
        repo
    }

    /// The temp dir that holds the repository and its `HOME`.
    pub fn dir(&self) -> &Path {
        &self.dir.0
    }

    /// Runs `git` in the root with the fixed, isolated environment.
    pub fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> String {
        let mut command = Command::new("git");
        command
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
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("run git");
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

/// `paths` as an indexed path set.
pub fn indexed(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_owned()).collect()
}

/// Every entry under `root`: its relative path, kind, length, mtime and,
/// for files, bytes. Two equal snapshots mean nothing was written.
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
