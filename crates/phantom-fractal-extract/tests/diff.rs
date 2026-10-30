//! Changed symbols (SPEC-w9 G2a, D8): the work tree against a base
//! commit, extracted in memory on both sides.
//!
//! Each repository is built by the `git` CLI in a temp dir: `git init`,
//! one `git fast-import` stream with fixed times and identity, and `git
//! reset --hard`, with no system or global config, a temp `HOME` and no
//! hooks.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{TempDir, copy_tree};
use phantom_fractal_extract::{
    ChangedSymbol, Diff, DiffError, FileOnly, diff, index, symbol_keys, walk,
};
use phantom_fractal_git::{Base, ChangesError, FileChange};
use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const T0: i64 = 1_750_000_000;
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

const LIB: &str = "pub mod a;\npub mod b;\n\npub fn top() {\n    a::helper();\n    b::run();\n}\n";
const A: &str = "pub fn helper() -> u32 {\n    1\n}\n\npub fn other() {}\n\npub struct S;\n\nimpl S {\n    pub fn m(&self) {}\n}\n";
const B: &str = "pub fn run() {\n    crate::a::other();\n}\n";
const MANIFEST: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

/// A repository whose `main` has one commit of the package, plus a
/// committed `.env.rs`, an ignored `gen/out.rs` and a `target/old.rs`.
struct Repo {
    dir: TempDir,
    home: TempDir,
}

impl Repo {
    fn new() -> Self {
        let repo = Self {
            dir: TempDir::new("diff-repo"),
            home: TempDir::new("diff-home"),
        };
        repo.git(&["init", "-q", "-b", "main"], None);
        repo.commit(
            "main",
            T0,
            &[
                ("Cargo.toml", MANIFEST),
                (".gitignore", "/gen/\n"),
                ("README.md", "readme\n"),
                ("src/lib.rs", LIB),
                ("src/a.rs", A),
                ("src/b.rs", B),
                (".env.rs", "pub fn secret() {}\n"),
                ("gen/out.rs", "pub fn generated() {}\n"),
                ("target/old.rs", "pub fn built() {}\n"),
            ],
        );
        repo.git(&["reset", "-q", "--hard", "main"], None);
        repo
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// One fast-import commit on `branch` at `time`, writing `files`.
    fn commit(&self, branch: &str, time: i64, files: &[(&str, &str)]) {
        let mut stream = format!(
            "commit refs/heads/{branch}\ncommitter Test <test@example.invalid> {time} +0000\ndata 7\nmessage\n"
        );
        if time > T0 {
            stream.push_str("from refs/heads/main^0\n");
        }
        for (path, body) in files {
            stream.push_str(&format!(
                "M 100644 inline {path}\ndata {}\n{body}\n",
                body.len()
            ));
        }
        self.git(&["fast-import", "--quiet"], Some(stream.as_bytes()));
    }

    fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> String {
        let mut child = Command::new("git")
            .current_dir(self.root())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.home.path())
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
        String::from_utf8(output.stdout).expect("utf-8")
    }

    fn rev(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev], None).trim().to_owned()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, text).expect("write");
    }

    fn diff(&self, base: &Base) -> Diff {
        let rust = RustLanguage::new().expect("query compiles");
        diff(self.root(), base, &rust)
            .expect("diff")
            .expect("a git root")
    }
}

fn changed(path: &str, key: &str, name: &str, change: FileChange) -> ChangedSymbol {
    ChangedSymbol {
        path: path.to_owned(),
        key: key.to_owned(),
        name: name.to_owned(),
        change,
    }
}

fn file(path: &str, change: FileChange) -> FileOnly {
    FileOnly {
        path: path.to_owned(),
        change,
    }
}

#[test]
fn a_clean_tree_changes_nothing() {
    let repo = Repo::new();
    let found = repo.diff(&Base::Head);
    assert_eq!(found.base_oid, repo.rev("HEAD"));
    assert_eq!(
        (found.changed, found.files_only, found.manifest_changed),
        (vec![], vec![], false)
    );
}

#[test]
fn an_edited_body_is_modified() {
    let repo = Repo::new();
    repo.write("src/a.rs", &A.replace("    1\n", "    2\n"));
    let found = repo.diff(&Base::Head);
    assert_eq!(
        found.changed,
        [changed(
            "src/a.rs",
            "src/a.rs#helper#function#0",
            "helper",
            FileChange::Modified
        )]
    );
    assert!(found.files_only.is_empty());
}

#[test]
fn an_edited_method_is_modified_under_its_method_key() {
    let repo = Repo::new();
    repo.write(
        "src/a.rs",
        &A.replace("pub fn m(&self) {}", "pub fn m(&self) { other(); }"),
    );
    assert_eq!(
        repo.diff(&Base::Head).changed,
        [changed(
            "src/a.rs",
            "src/a.rs#S::m#method#0",
            "m",
            FileChange::Modified
        )]
    );
}

#[test]
fn an_added_fn_and_a_removed_fn() {
    let repo = Repo::new();
    repo.write("src/b.rs", &format!("{B}\npub fn more() {{}}\n"));
    repo.write("src/a.rs", &A.replace("pub fn other() {}\n\n", ""));
    assert_eq!(
        repo.diff(&Base::Head).changed,
        [
            changed(
                "src/a.rs",
                "src/a.rs#other#function#0",
                "other",
                FileChange::Removed
            ),
            changed(
                "src/b.rs",
                "src/b.rs#more#function#0",
                "more",
                FileChange::Added
            ),
        ]
    );
}

#[test]
fn a_fn_moved_to_another_file_is_removed_there_and_added_here() {
    let repo = Repo::new();
    repo.write("src/a.rs", &A.replace("pub fn other() {}\n\n", ""));
    repo.write("src/b.rs", &format!("pub fn other() {{}}\n\n{B}"));
    assert_eq!(
        repo.diff(&Base::Head).changed,
        [
            changed(
                "src/a.rs",
                "src/a.rs#other#function#0",
                "other",
                FileChange::Removed
            ),
            changed(
                "src/b.rs",
                "src/b.rs#other#function#0",
                "other",
                FileChange::Added
            ),
        ]
    );
}

#[test]
fn a_fn_moved_within_its_file_is_no_symbol_change() {
    let repo = Repo::new();
    repo.write(
        "src/a.rs",
        &format!(
            "pub fn other() {{}}\n\n{}",
            A.replace("pub fn other() {}\n\n", "")
        ),
    );
    let found = repo.diff(&Base::Head);
    assert!(found.changed.is_empty(), "{:?}", found.changed);
    assert_eq!(found.files_only, [file("src/a.rs", FileChange::Modified)]);
}

#[test]
fn a_removed_file_removes_its_symbols() {
    let repo = Repo::new();
    fs::remove_file(repo.root().join("src/b.rs")).expect("delete");
    assert_eq!(
        repo.diff(&Base::Head).changed,
        [changed(
            "src/b.rs",
            "src/b.rs#run#function#0",
            "run",
            FileChange::Removed
        )]
    );
}

#[test]
fn a_non_rust_change_is_listed_per_file() {
    let repo = Repo::new();
    repo.write("README.md", "readme, edited\n");
    repo.write("docs/new.md", "new\n");
    let found = repo.diff(&Base::Head);
    assert!(found.changed.is_empty());
    assert_eq!(
        found.files_only,
        [
            file("README.md", FileChange::Modified),
            file("docs/new.md", FileChange::Added)
        ]
    );
    assert!(!found.manifest_changed);
}

#[test]
fn a_manifest_edit_sets_manifest_changed() {
    let repo = Repo::new();
    repo.write("Cargo.toml", &MANIFEST.replace("0.1.0", "0.2.0"));
    let found = repo.diff(&Base::Head);
    assert!(found.manifest_changed);
    assert_eq!(found.files_only, [file("Cargo.toml", FileChange::Modified)]);
}

#[test]
fn files_the_index_never_keeps_are_no_change() {
    let repo = Repo::new();
    // Committed but secret, ignored, or under a pruned `target`.
    fs::remove_file(repo.root().join(".env.rs")).expect("delete secret");
    repo.write("gen/out.rs", "pub fn regenerated() {}\n");
    fs::remove_file(repo.root().join("target/old.rs")).expect("delete built");
    repo.write("gen/more.rs", "pub fn more() {}\n");
    let found = repo.diff(&Base::Head);
    assert!(found.changed.is_empty(), "{:?}", found.changed);
    assert!(found.files_only.is_empty(), "{:?}", found.files_only);
}

#[test]
fn a_file_the_index_stops_keeping_removes_its_symbols() {
    // Still in the tree, but no longer UTF-8, or now over the size cap:
    // an index drops it, so its symbols are removed.
    let repo = Repo::new();
    fs::write(repo.root().join("src/b.rs"), b"pub fn run() {} // \xff\n").expect("not utf-8");
    let big = format!("{A}{}", "// filler\n".repeat(120_000));
    repo.write("src/a.rs", &big);
    let found = repo.diff(&Base::Head);
    let removed: Vec<(&str, FileChange)> = found
        .changed
        .iter()
        .map(|symbol| (symbol.key.as_str(), symbol.change))
        .collect();
    assert_eq!(
        removed,
        [
            ("src/a.rs#S#struct#0", FileChange::Removed),
            ("src/a.rs#S::m#method#0", FileChange::Removed),
            ("src/a.rs#helper#function#0", FileChange::Removed),
            ("src/a.rs#other#function#0", FileChange::Removed),
            ("src/b.rs#run#function#0", FileChange::Removed),
        ]
    );
}

#[test]
fn a_short_oid_of_an_annotated_tag_names_its_commit() {
    let repo = Repo::new();
    let first = repo.rev("main");
    let stream = format!(
        "tag annotated\nfrom {first}\ntagger Test <test@example.invalid> {} +0000\ndata 4\nnote\n",
        T0 + 30
    );
    repo.git(&["fast-import", "--quiet"], Some(stream.as_bytes()));
    let tag_object = repo.rev("refs/tags/annotated");
    assert_ne!(tag_object, first);
    let found = repo.diff(&Base::Ref(tag_object[..9].to_owned()));
    assert_eq!(found.base_oid, first);
}

#[test]
fn against_a_tag_a_branch_and_a_short_oid() {
    let repo = Repo::new();
    let first = repo.rev("main");
    repo.git(&["tag", "v1", "main"], None);
    repo.commit(
        "main",
        T0 + 60,
        &[("src/b.rs", "pub fn run() {}\n\npub fn later() {}\n")],
    );
    repo.git(&["reset", "-q", "--hard", "main"], None);
    repo.git(&["branch", "first", "v1"], None);
    // Against HEAD the tree is clean; against the first commit, `run`
    // changed and `later` is new.
    assert!(repo.diff(&Base::Head).changed.is_empty());
    let want = [
        changed(
            "src/b.rs",
            "src/b.rs#later#function#0",
            "later",
            FileChange::Added,
        ),
        changed(
            "src/b.rs",
            "src/b.rs#run#function#0",
            "run",
            FileChange::Modified,
        ),
    ];
    for name in ["v1", "first", &first[..7], &first[..10]] {
        let found = repo.diff(&Base::Ref(name.to_owned()));
        assert_eq!(found.base_oid, first, "{name}");
        assert_eq!(found.changed, want, "{name}");
    }
}

#[test]
fn an_unknown_base_is_an_error() {
    let repo = Repo::new();
    let rust = RustLanguage::new().expect("query compiles");
    for name in ["nope", "abc12", "HEAD~1"] {
        match diff(repo.root(), &Base::Ref(name.to_owned()), &rust) {
            Err(DiffError::Changes(ChangesError::UnknownBase)) => {}
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn a_root_without_git_is_none() {
    let dir = TempDir::new("diff-plain");
    fs::write(dir.path().join("lib.rs"), "pub fn f() {}\n").expect("write");
    let rust = RustLanguage::new().expect("query compiles");
    assert!(
        diff(dir.path(), &Base::Head, &rust)
            .expect("diff")
            .is_none()
    );
}

#[test]
fn the_keys_are_the_ones_the_store_gives() {
    let rust = RustLanguage::new().expect("query compiles");
    let mut fixtures: Vec<PathBuf> = fs::read_dir(FIXTURES)
        .expect("fixtures")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    fixtures.sort();
    let mut checked = 0;
    for fixture in fixtures {
        let tree = TempDir::new("diff-keys");
        copy_tree(&fixture, tree.path());
        let db = TempDir::new("diff-keys-db");
        let mut store = Store::open(&db.path().join("graph.db")).expect("open store");
        index(tree.path(), &mut store, &rust).expect("index");
        let mut keys: BTreeMap<String, ()> = BTreeMap::new();
        for file in walk(tree.path()).expect("walk").files {
            if !file.path.ends_with(".rs") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file.abs_path) else {
                continue;
            };
            let extract = rust.extract(&text).expect("extract");
            for (key, ..) in symbol_keys(&file.path, &extract) {
                assert!(
                    store.symbol_by_stable_key(&key).expect("lookup").is_some(),
                    "{}: no stored symbol {key}",
                    fixture.display()
                );
                assert!(keys.insert(key, ()).is_none());
            }
        }
        assert_eq!(
            keys.len() as i64,
            store.row_count("symbols").expect("count"),
            "{}",
            fixture.display()
        );
        checked += keys.len();
    }
    assert!(checked > 100, "{checked}");
}

#[test]
fn diff_writes_nothing() {
    let repo = Repo::new();
    repo.write("src/a.rs", &A.replace("    1\n", "    2\n"));
    let before = common_snapshot(&[repo.root(), repo.home.path()]);
    let first = repo.diff(&Base::Head);
    let second = repo.diff(&Base::Ref("main".to_owned()));
    assert_eq!(first, second);
    assert_eq!(common_snapshot(&[repo.root(), repo.home.path()]), before);
}

/// Every entry under each of `roots`: path, length, mtime and bytes.
fn common_snapshot(roots: &[&Path]) -> Vec<(PathBuf, u64, std::time::SystemTime, Vec<u8>)> {
    let mut out = Vec::new();
    for root in roots {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read dir") {
                let path = entry.expect("entry").path();
                let meta = fs::symlink_metadata(&path).expect("meta");
                if meta.is_dir() {
                    stack.push(path.clone());
                }
                let bytes = if meta.is_file() {
                    fs::read(&path).expect("read")
                } else {
                    Vec::new()
                };
                out.push((path, meta.len(), meta.modified().expect("mtime"), bytes));
            }
        }
    }
    out.sort();
    out
}
