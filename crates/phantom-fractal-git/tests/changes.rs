//! `changes` (SPEC-w9 G2a, D7): the work tree against HEAD or a named
//! base, from object, tree and reference reads only.
//!
//! Each repository is built by the `git` CLI in a temp dir with fixed
//! times and identity, no system or global config, a temp `HOME` and no
//! hooks. The tests read the work tree themselves, as the extract crate's
//! caller does, and pass every file as kept.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use phantom_fractal_git::{Base, Changed, ChangesError, FileChange, MIN_OID_HEX, changes, head};
use support::{Op, Repo, Script, snapshot};

const T0: i64 = 1_750_000_000;

/// `main`: a first commit with `a.rs`, `b.rs`, `notes.txt` and a link,
/// and a second that edits `a.rs`. `old` points at the first.
fn repo() -> Repo {
    let mut script = Script::default();
    script.commit(
        "main",
        T0,
        &[
            Op::Write("src/a.rs", "pub fn a() {}\n"),
            Op::Write("src/b.rs", "pub fn b() {}\n"),
            Op::Write("notes.txt", "notes\n"),
            Op::Link("src/link.rs", "a.rs"),
        ],
    );
    script.commit(
        "main",
        T0 + 60,
        &[Op::Write("src/a.rs", "pub fn a() { b(); }\n")],
    );
    let repo = Repo::new("changes", &script);
    repo.git(&["branch", "old", "main~1"], None);
    repo
}

/// Every regular file under `root` outside `.git`, by relative path.
fn worktree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("entry").path();
            let meta = fs::symlink_metadata(&path).expect("meta");
            if meta.is_dir() {
                if path.file_name().is_some_and(|name| name != ".git") {
                    stack.push(path);
                }
            } else if meta.is_file() {
                let rel = path.strip_prefix(root).expect("under root");
                out.insert(
                    rel.to_string_lossy().into_owned(),
                    fs::read(&path).expect("read"),
                );
            }
        }
    }
    out
}

fn keep_all(_: &str, _: u64) -> bool {
    true
}

fn run(repo: &Repo, base: &Base) -> Result<Vec<Changed>, ChangesError> {
    let found = changes(&repo.root, base, &worktree(&repo.root), &keep_all)?.expect("a git root");
    Ok(found.files)
}

fn summary(files: &[Changed]) -> Vec<(&str, FileChange, Option<&[u8]>)> {
    files
        .iter()
        .map(|file| (file.path.as_str(), file.change, file.base.as_deref()))
        .collect()
}

#[test]
fn a_clean_tree_has_no_changes_against_head() {
    let repo = repo();
    let found = changes(&repo.root, &Base::Head, &worktree(&repo.root), &keep_all)
        .expect("changes")
        .expect("a git root");
    assert_eq!(found.base_oid, repo.rev("HEAD"));
    assert_eq!(
        Some(found.base_oid.clone()),
        head(&repo.root).expect("head")
    );
    assert!(found.files.is_empty(), "{:?}", found.files);
}

#[test]
fn edits_additions_and_removals_against_head_by_raw_bytes() {
    let repo = repo();
    fs::write(
        repo.root.join("src/a.rs"),
        "pub fn a() { b(); }\n// edited\n",
    )
    .expect("edit");
    fs::remove_file(repo.root.join("src/b.rs")).expect("delete");
    fs::write(repo.root.join("src/new.rs"), "pub fn new() {}\n").expect("add");
    // The same bytes, rewritten: no change.
    fs::write(repo.root.join("notes.txt"), "notes\n").expect("rewrite");
    let files = run(&repo, &Base::Head).expect("changes");
    assert_eq!(
        summary(&files),
        [
            (
                "src/a.rs",
                FileChange::Modified,
                Some(&b"pub fn a() { b(); }\n"[..])
            ),
            (
                "src/b.rs",
                FileChange::Removed,
                Some(&b"pub fn b() {}\n"[..])
            ),
            ("src/new.rs", FileChange::Added, None),
        ]
    );
}

#[test]
fn a_move_is_a_removal_and_an_addition() {
    let repo = repo();
    fs::rename(repo.root.join("src/b.rs"), repo.root.join("src/c.rs")).expect("move");
    let files = run(&repo, &Base::Head).expect("changes");
    assert_eq!(
        summary(&files),
        [
            (
                "src/b.rs",
                FileChange::Removed,
                Some(&b"pub fn b() {}\n"[..])
            ),
            ("src/c.rs", FileChange::Added, None),
        ]
    );
}

#[test]
fn a_branch_a_tag_an_annotated_tag_and_an_oid_name_the_same_base() {
    let repo = repo();
    let old = repo.rev("old");
    // A lightweight tag, and an annotated one written with a fixed date.
    repo.git(&["tag", "light", "old"], None);
    let stream = format!(
        "tag annotated\nfrom {old}\ntagger Test <test@example.invalid> {} +0000\ndata 4\nnote\n",
        T0 + 120
    );
    repo.git(&["fast-import", "--quiet"], Some(stream.as_bytes()));
    let tag_object = repo.rev("refs/tags/annotated");
    assert_ne!(tag_object, old);
    for name in [
        "old".to_owned(),
        "light".to_owned(),
        "annotated".to_owned(),
        tag_object[..MIN_OID_HEX].to_owned(),
        old[..MIN_OID_HEX].to_owned(),
        old[..12].to_ascii_uppercase(),
        old.clone(),
    ] {
        let found = changes(
            &repo.root,
            &Base::Ref(name.clone()),
            &worktree(&repo.root),
            &keep_all,
        )
        .expect("changes")
        .expect("a git root");
        assert_eq!(found.base_oid, old, "{name}");
        assert_eq!(
            summary(&found.files),
            [(
                "src/a.rs",
                FileChange::Modified,
                Some(&b"pub fn a() {}\n"[..])
            )],
            "{name}"
        );
    }
}

#[test]
fn an_unknown_base_is_refused() {
    let repo = repo();
    let blob = repo
        .git(&["rev-parse", "HEAD:src/a.rs"], None)
        .trim()
        .to_owned();
    let tree = repo
        .git(&["rev-parse", "HEAD^{tree}"], None)
        .trim()
        .to_owned();
    for name in [
        "nope",
        "",
        "main~1",
        "../main",
        "refs/heads/main",
        &repo.rev("HEAD")[..MIN_OID_HEX - 1],
        "zzzzzzz",
        "0000000",
        &blob[..10],
        &tree,
    ] {
        assert_eq!(
            run(&repo, &Base::Ref(name.to_owned())),
            Err(ChangesError::UnknownBase),
            "{name:?}"
        );
    }
}

#[test]
fn a_branch_and_a_tag_of_one_name_on_two_commits_are_ambiguous() {
    let repo = repo();
    repo.git(&["tag", "both", "main"], None);
    repo.git(&["branch", "both", "old"], None);
    assert_eq!(
        run(&repo, &Base::Ref("both".to_owned())),
        Err(ChangesError::AmbiguousBase)
    );
    // On one commit, they agree.
    repo.git(&["tag", "same", "old"], None);
    repo.git(&["branch", "same", "old"], None);
    let found = changes(
        &repo.root,
        &Base::Ref("same".to_owned()),
        &worktree(&repo.root),
        &keep_all,
    )
    .expect("changes")
    .expect("a git root");
    assert_eq!(found.base_oid, repo.rev("old"));
}

#[test]
fn a_base_file_the_index_would_not_keep_is_no_removal_and_its_path_is_added() {
    let repo = repo();
    fs::remove_file(repo.root.join("src/b.rs")).expect("delete");
    let refuse_b = |path: &str, _: u64| path != "src/b.rs" && path != "src/a.rs";
    fs::write(
        repo.root.join("src/a.rs"),
        "pub fn a() { b(); }\n// edited\n",
    )
    .expect("edit");
    let found = changes(&repo.root, &Base::Head, &worktree(&repo.root), &refuse_b)
        .expect("changes")
        .expect("a git root");
    assert_eq!(
        summary(&found.files),
        [("src/a.rs", FileChange::Added, None)]
    );
}

#[test]
fn a_symlink_in_the_base_is_never_counted() {
    let repo = repo();
    fs::remove_file(repo.root.join("src/link.rs")).expect("remove link");
    assert_eq!(run(&repo, &Base::Head).expect("changes"), []);
}

#[test]
fn the_git_index_is_never_read() {
    let repo = repo();
    fs::write(repo.root.join("src/a.rs"), "pub fn a() {}\n// staged? no\n").expect("edit");
    let index = repo.root.join(".git/index");
    assert!(index.is_file(), "checkout writes an index");
    let want = run(&repo, &Base::Head).expect("changes");
    fs::set_permissions(&index, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    assert_eq!(
        run(&repo, &Base::Head).expect("changes, index unreadable"),
        want
    );
    fs::set_permissions(&index, fs::Permissions::from_mode(0o644)).expect("chmod back");
    fs::remove_file(&index).expect("delete index");
    assert_eq!(run(&repo, &Base::Head).expect("changes, no index"), want);
}

#[test]
fn not_a_git_root_is_none() {
    let repo = repo();
    let empty = support::TempDir::new("changes-plain");
    for root in [
        empty.0.as_path(),
        &repo.root.join("src"),
        &repo.root.join(".git"),
    ] {
        let found = changes(root, &Base::Head, &BTreeMap::new(), &keep_all).expect("changes");
        assert_eq!(found, None, "{}", root.display());
        assert_eq!(head(root).expect("head"), None);
    }
}

#[test]
fn changes_writes_nothing() {
    let repo = repo();
    fs::write(repo.root.join("src/a.rs"), "pub fn a() {}\n// edited\n").expect("edit");
    let before = snapshot(repo.dir());
    let first = run(&repo, &Base::Ref("old".to_owned())).expect("changes");
    let second = run(&repo, &Base::Head).expect("changes");
    assert_eq!(snapshot(repo.dir()), before);
    assert_eq!(
        first,
        run(&repo, &Base::Ref("old".to_owned())).expect("again")
    );
    assert_eq!(second.len(), 1);
}
