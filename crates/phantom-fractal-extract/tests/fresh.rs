//! The freshness probe (SPEC-w9 G0b). Each case indexes a tree, makes one
//! change, and checks the probe's lists and flags; then it runs the index
//! pass and checks that the pass did exactly what the probe said, and that
//! a probe after it is fresh.

mod common;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::{FixtureRepo, TempDir, rust_bytes, write};
use phantom_fractal_extract::{Freshness, IndexReport, MAX_FILE_BYTES, index, probe};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

/// A tree, a store beside it, and the extractor.
struct Probed {
    root: PathBuf,
    db_dir: TempDir,
    store: Store,
    rust: RustLanguage,
}

impl Probed {
    fn new(root: &Path) -> Self {
        let db_dir = TempDir::new("fresh-db");
        let store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
        let rust = RustLanguage::new().expect("query compiles");
        let mut out = Self {
            root: root.to_path_buf(),
            db_dir,
            store,
            rust,
        };
        out.index();
        out
    }

    /// One index pass, then stores HEAD as G0c's `index` will.
    fn index(&mut self) -> IndexReport {
        let report = index(&self.root, &mut self.store, &self.rust).expect("index");
        let head = phantom_fractal_git::head(&self.root).expect("head");
        self.store
            .set_index_head(head.as_deref())
            .expect("set index head");
        report
    }

    fn probe(&self) -> Freshness {
        probe(&self.root, &self.store).expect("probe")
    }

    /// The probe equals `want`; the index pass then writes, removes and
    /// rebuilds exactly as the probe said; and the probe after it is fresh.
    fn assert_probe(&mut self, want: &Freshness) {
        let got = self.probe();
        assert_eq!(&got, want);
        assert_eq!(got.stale(), stale(want));
        let before: Vec<String> = self.store.file_paths().expect("paths");
        let report = self.index();
        let mut written = report.written.clone();
        written.sort();
        let mut changed: Vec<String> = got.added.iter().chain(&got.modified).cloned().collect();
        changed.sort();
        assert_eq!(written, changed, "written");
        for path in &got.added {
            assert!(!before.contains(path), "{path} was stored");
        }
        for path in &got.modified {
            assert!(before.contains(path), "{path} was not stored");
        }
        let mut removed = report.removed.clone();
        removed.sort();
        assert_eq!(removed, got.removed, "removed");
        let rebuilt = !changed.is_empty() || !got.removed.is_empty() || got.fingerprint_changed;
        assert_eq!(report.graph.is_some(), rebuilt, "rebuilt");
        let after = self.probe();
        assert!(!after.stale(), "stale after index: {after:?}");
        assert_eq!(after.head, got.head);
    }
}

/// [`Freshness::stale`], written out.
fn stale(f: &Freshness) -> bool {
    !(f.added.is_empty()
        && f.modified.is_empty()
        && f.removed.is_empty()
        && !f.fingerprint_changed
        && f.head == f.index_head)
}

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|path| (*path).to_owned()).collect()
}

/// A fresh probe on a git-free root.
fn fresh() -> Freshness {
    Freshness::default()
}

/// A two-member workspace with no git.
fn workspace() -> TempDir {
    let dir = TempDir::new("fresh");
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        b"[workspace]\nmembers = [\"app\", \"base\"]\n",
    );
    write(
        root,
        "app/Cargo.toml",
        b"[package]\nname = \"app\"\n\n[dependencies]\nbase = { path = \"../base\" }\n",
    );
    write(
        root,
        "app/src/lib.rs",
        b"pub mod util;\n\npub fn run() {\n    base::shared();\n}\n",
    );
    write(
        root,
        "app/src/util.rs",
        b"pub fn helper() -> u32 {\n    1\n}\n",
    );
    write(root, "base/Cargo.toml", b"[package]\nname = \"base\"\n");
    write(root, "base/src/lib.rs", b"pub fn shared() {}\n");
    write(root, ".gitignore", b"/scratch/\n*.generated.rs\n");
    write(root, "README.md", b"a workspace\n");
    dir
}

/// Moves the mtime of `path` one second forward, keeping its bytes.
fn touch(path: &Path) {
    let file = File::options().write(true).open(path).expect("open");
    let mtime = file.metadata().expect("meta").modified().expect("mtime");
    file.set_modified(mtime + Duration::from_secs(1))
        .expect("set mtime");
}

#[test]
fn a_fresh_index_is_not_stale() {
    let dir = workspace();
    let s = Probed::new(dir.path());
    let got = s.probe();
    assert_eq!(got, fresh());
    assert!(!got.stale());
}

#[test]
fn the_indexer_fixture_is_fresh_after_its_index() {
    // Excluded, gitignored, oversized, symlinked, hard-linked, secret and
    // `.git` paths: none is reported, and the fingerprint matches.
    let repo = FixtureRepo::new();
    let s = Probed::new(repo.root());
    assert_eq!(s.probe(), fresh());
}

#[test]
fn an_added_file_is_added() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(dir.path(), "app/src/extra.rs", b"pub fn extra() {}\n");
    s.assert_probe(&Freshness {
        added: paths(&["app/src/extra.rs"]),
        ..fresh()
    });
}

#[test]
fn files_added_in_several_directories_are_sorted_by_path() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    // The walk sorts each directory by file name, so it reaches
    // `app/src/a/b.rs` before `app/src/a.rs`; the list is sorted by path.
    for path in [
        "app/src/z.rs",
        "app/src/a/b.rs",
        "app/src/a.rs",
        "base/a.rs",
    ] {
        write(dir.path(), path, b"pub fn x() {}\n");
    }
    s.assert_probe(&Freshness {
        added: paths(&[
            "app/src/a.rs",
            "app/src/a/b.rs",
            "app/src/z.rs",
            "base/a.rs",
        ]),
        ..fresh()
    });
}

#[test]
fn an_edit_is_modified() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(
        dir.path(),
        "app/src/util.rs",
        b"pub fn helper() -> u32 {\n    2\n}\n",
    );
    // Same size: the mtime alone sends it to the hash.
    touch(&dir.path().join("app/src/util.rs"));
    s.assert_probe(&Freshness {
        modified: paths(&["app/src/util.rs"]),
        ..fresh()
    });
}

#[test]
fn an_edit_that_keeps_the_mtime_is_seen_by_its_size() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    let path = dir.path().join("app/src/util.rs");
    let mtime = fs::metadata(&path)
        .expect("meta")
        .modified()
        .expect("mtime");
    fs::write(&path, b"pub fn helper() -> u32 {\n    20\n}\n").expect("edit");
    File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(mtime)
        .expect("keep mtime");
    s.assert_probe(&Freshness {
        modified: paths(&["app/src/util.rs"]),
        ..fresh()
    });
}

#[test]
fn a_deleted_file_is_removed() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    fs::remove_file(dir.path().join("app/src/util.rs")).expect("delete");
    s.assert_probe(&Freshness {
        removed: paths(&["app/src/util.rs"]),
        ..fresh()
    });
}

#[test]
fn a_touch_alone_is_not_stale() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    for path in ["app/src/util.rs", "base/src/lib.rs", "Cargo.toml"] {
        touch(&dir.path().join(path));
    }
    s.assert_probe(&fresh());
    // The pass wrote nothing, so the stored mtime is still the old one,
    // and the probe still answers fresh.
    assert_eq!(s.probe(), fresh());
}

#[test]
fn a_manifest_edit_changes_only_the_fingerprint() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(
        dir.path(),
        "app/Cargo.toml",
        b"[package]\nname = \"app\"\n\n[dependencies]\n",
    );
    s.assert_probe(&Freshness {
        fingerprint_changed: true,
        ..fresh()
    });
}

#[test]
fn a_new_member_manifest_changes_the_fingerprint_and_adds_its_sources() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(
        dir.path(),
        "tool/Cargo.toml",
        b"[package]\nname = \"tool\"\n",
    );
    write(dir.path(), "tool/src/main.rs", b"fn main() {}\n");
    s.assert_probe(&Freshness {
        added: paths(&["tool/src/main.rs"]),
        fingerprint_changed: true,
        ..fresh()
    });
}

#[test]
fn ignored_files_are_never_reported() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    let root = dir.path();
    write(root, "scratch/notes.rs", b"pub fn scratch() {}\n");
    write(root, "app/src/out.generated.rs", b"pub fn generated() {}\n");
    write(root, "target/debug/build.rs", b"pub fn built() {}\n");
    write(root, "app/target/x.rs", b"pub fn built() {}\n");
    write(root, ".git/hooks.rs", b"pub fn in_git() {}\n");
    write(root, "README.md", b"a workspace, edited\n");
    write(root, "app/src/notes.txt", b"not rust\n");
    s.assert_probe(&fresh());
}

#[test]
fn a_file_the_walk_skips_changes_only_the_fingerprint() {
    // The indexer records every skipped `.rs` path in the fingerprint, so
    // a new secret-named, oversized or symlinked `.rs` file rebuilds
    // without being added.
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    let root = dir.path();
    write(root, "app/src/.env.rs", b"pub fn env() {}\n");
    write(
        root,
        "app/src/id_codec",
        b"not rust, so not in the fingerprint\n",
    );
    write(
        root,
        "app/src/big.rs",
        &rust_bytes(b"pub fn big() {}\n", MAX_FILE_BYTES + 1),
    );
    std::os::unix::fs::symlink(root.join("app/src/util.rs"), root.join("app/src/alias.rs"))
        .expect("symlink");
    s.assert_probe(&Freshness {
        fingerprint_changed: true,
        ..fresh()
    });
}

#[test]
fn a_stored_file_that_stops_being_utf8_is_removed() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(
        dir.path(),
        "app/src/util.rs",
        b"pub fn helper() {} // \xff\n",
    );
    s.assert_probe(&Freshness {
        removed: paths(&["app/src/util.rs"]),
        fingerprint_changed: true,
        ..fresh()
    });
}

#[test]
fn a_stored_file_that_grows_past_the_cap_is_removed() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    write(
        dir.path(),
        "app/src/util.rs",
        &rust_bytes(b"pub fn helper() {}\n", MAX_FILE_BYTES + 1),
    );
    s.assert_probe(&Freshness {
        removed: paths(&["app/src/util.rs"]),
        fingerprint_changed: true,
        ..fresh()
    });
}

#[test]
fn add_edit_and_delete_together_give_each_list() {
    let dir = workspace();
    let mut s = Probed::new(dir.path());
    let root = dir.path();
    write(root, "base/src/more.rs", b"pub fn more() {}\n");
    write(
        root,
        "app/src/lib.rs",
        b"pub mod util;\n\npub fn run() {}\n",
    );
    fs::remove_file(root.join("base/src/lib.rs")).expect("delete");
    s.assert_probe(&Freshness {
        added: paths(&["base/src/more.rs"]),
        modified: paths(&["app/src/lib.rs"]),
        removed: paths(&["base/src/lib.rs"]),
        ..fresh()
    });
}

/// Runs `git` in `root` with no system or global config, a temp `HOME`,
/// no hooks and a fixed identity.
fn git(root: &Path, home: &Path, args: &[&str], stdin: Option<&[u8]>) {
    let mut child = Command::new("git")
        .current_dir(root)
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

/// One fast-import commit on `main` at a fixed time, writing `files`.
fn commit(time: i64, files: &[(&str, &str)]) -> String {
    let mut text = format!(
        "commit refs/heads/main\ncommitter Test <test@example.invalid> {time} +0000\ndata 7\nmessage\n"
    );
    if time > 1_750_000_000 {
        text.push_str("from refs/heads/main^0\n");
    }
    for (path, body) in files {
        text.push_str(&format!(
            "M 100644 inline {path}\ndata {}\n{body}\n",
            body.len()
        ));
    }
    text
}

/// A git repo whose work tree is the workspace, with one commit.
struct GitRepo {
    dir: TempDir,
    home: TempDir,
}

impl GitRepo {
    fn new() -> Self {
        let dir = TempDir::new("fresh-git");
        let home = TempDir::new("fresh-home");
        let repo = Self { dir, home };
        repo.git(&["init", "-q", "-b", "main"], None);
        let first = commit(
            1_750_000_000,
            &[
                ("Cargo.toml", "[package]\nname = \"one\"\n"),
                ("src/lib.rs", "pub fn one() {}\n"),
                ("README.md", "one\n"),
            ],
        );
        repo.git(&["fast-import", "--quiet"], Some(first.as_bytes()));
        repo.git(&["reset", "-q", "--hard", "main"], None);
        repo
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str], stdin: Option<&[u8]>) {
        git(self.root(), self.home.path(), args, stdin);
    }
}

#[test]
fn a_head_move_alone_is_stale_with_empty_lists() {
    let repo = GitRepo::new();
    let mut s = Probed::new(repo.root());
    let first = s.probe();
    assert!(!first.stale(), "{first:?}");
    let first_head = first.head.clone().expect("a git root");
    assert_eq!(first.index_head.as_deref(), Some(first_head.as_str()));

    // A second commit that changes only a file the index does not keep.
    let second = commit(1_750_000_100, &[("README.md", "two\n")]);
    repo.git(&["fast-import", "--quiet"], Some(second.as_bytes()));
    repo.git(&["reset", "-q", "--hard", "main"], None);
    let moved = s.probe();
    let head = moved.head.clone().expect("a git root");
    assert_ne!(head, first_head);
    s.assert_probe(&Freshness {
        head: Some(head),
        index_head: Some(first_head),
        ..fresh()
    });
}

#[test]
fn a_detached_head_and_a_source_change_are_both_reported() {
    let repo = GitRepo::new();
    let second = commit(1_750_000_100, &[("src/lib.rs", "pub fn two() {}\n")]);
    repo.git(&["fast-import", "--quiet"], Some(second.as_bytes()));
    repo.git(&["reset", "-q", "--hard", "main"], None);
    let mut s = Probed::new(repo.root());
    let tip = s.probe().head.expect("a git root");
    repo.git(&["checkout", "-q", "--detach", "main~1"], None);
    let back = s.probe();
    let base = back.head.clone().expect("a git root");
    assert_ne!(base, tip);
    s.assert_probe(&Freshness {
        modified: paths(&["src/lib.rs"]),
        head: Some(base),
        index_head: Some(tip),
        ..fresh()
    });
}

#[test]
fn a_store_from_before_index_head_is_stale_on_a_git_root() {
    // A W8 database has no `index_head`; its first W9 pass writes it.
    let repo = GitRepo::new();
    let s = Probed::new(repo.root());
    let mut store = s.store;
    store.set_index_head(None).expect("clear index head");
    let got = probe(repo.root(), &store).expect("probe");
    assert!(got.head.is_some());
    assert_eq!(got.index_head, None);
    assert!(got.stale());
}

#[test]
fn a_root_without_git_has_no_head() {
    let dir = workspace();
    let s = Probed::new(dir.path());
    let got = s.probe();
    assert_eq!((got.head, got.index_head), (None, None));
}

/// Every entry under `dir`: kind, length, mtime and bytes (or a link's
/// target).
fn snapshot(dir: &Path, out: &mut BTreeMap<PathBuf, (u32, u64, i64, i64, Vec<u8>)>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).expect("meta");
        let bytes = if meta.file_type().is_symlink() {
            fs::read_link(&path)
                .expect("link")
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if meta.is_dir() {
            Vec::new()
        } else {
            fs::read(&path).expect("read")
        };
        out.insert(
            path.clone(),
            (
                meta.mode(),
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
                bytes,
            ),
        );
        if meta.is_dir() {
            snapshot(&path, out);
        }
    }
}

#[test]
fn the_probe_writes_nothing() {
    let repo = GitRepo::new();
    // Changes of every kind, so the probe reads, hashes and fingerprints.
    write(repo.root(), "src/new.rs", b"pub fn new() {}\n");
    let s = Probed::new(repo.root());
    write(repo.root(), "src/lib.rs", b"pub fn edited() {}\n");
    write(repo.root(), "Cargo.toml", b"[package]\nname = \"edited\"\n");
    fs::remove_file(repo.root().join("src/new.rs")).expect("delete");
    touch(&repo.root().join("README.md"));
    let mut before = BTreeMap::new();
    snapshot(repo.root(), &mut before);
    snapshot(repo.home.path(), &mut before);
    for name in ["graph.db", "graph.db-wal"] {
        let path = s.db_dir.path().join(name);
        let meta = fs::metadata(&path).expect("db file");
        let entry = (
            meta.mode(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            fs::read(&path).expect("read"),
        );
        before.insert(path, entry);
    }
    let first = s.probe();
    let second = s.probe();
    assert_eq!(first, second);
    assert!(first.stale());
    let mut after = BTreeMap::new();
    snapshot(repo.root(), &mut after);
    snapshot(repo.home.path(), &mut after);
    for name in ["graph.db", "graph.db-wal"] {
        let path = s.db_dir.path().join(name);
        let meta = fs::metadata(&path).expect("db file");
        let entry = (
            meta.mode(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec(),
            fs::read(&path).expect("read"),
        );
        after.insert(path, entry);
    }
    assert!(before == after, "the probe changed a file");
    // The db dir holds no file the probe made.
    let names: Vec<String> = fs::read_dir(s.db_dir.path())
        .expect("db dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        names.iter().all(|name| name.starts_with("graph.db")),
        "{names:?}"
    );
}
