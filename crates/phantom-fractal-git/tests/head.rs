//! `head`: HEAD's oid without a walk (SPEC-w8 G3a, the G3 contract).

mod support;

use std::fs;
use std::path::PathBuf;

use phantom_fractal_git::{Window, head, mine};
use support::{DAY, HEAD, Op, Repo, Script, TempDir, indexed, snapshot};

fn script() -> Script {
    let mut script = Script::default();
    let first = script.commit(
        "main",
        HEAD - 2 * DAY,
        &[Op::Write("src/lib.rs", "1\n"), Op::Write("a.rs", "1\n")],
    );
    let tip = script.branch("side", first, HEAD - DAY, &[Op::Write("b.rs", "1\n")]);
    script.commit("main", HEAD - DAY, &[Op::Write("a.rs", "2\n")]);
    script.merge("main", tip, HEAD, &[Op::Write("b.rs", "1\n")]);
    script
}

#[test]
fn head_is_the_text_the_scan_stores() {
    let repo = Repo::new("head", &script());
    let found = head(&repo.root).expect("head").expect("a git root");
    assert_eq!(found, repo.rev("HEAD"));
    let scan = mine(&repo.root, &Window::default(), &indexed(&["a.rs", "b.rs"]))
        .expect("mine")
        .expect("a git root");
    assert_eq!(scan.last_oid.as_deref(), Some(found.as_str()));

    // A detached HEAD answers the commit it points at.
    repo.git(&["checkout", "-q", "--detach", "main~1"], None);
    assert_eq!(head(&repo.root), Ok(Some(repo.rev("main~1"))));
}

#[test]
fn head_is_none_exactly_where_mine_is_none() {
    let all = indexed(&["a.rs"]);
    let repo = Repo::new("head-none", &script());
    let plain = TempDir::new("head-plain");
    let fake = TempDir::new("head-fake");
    fs::write(fake.0.join(".git"), "not a repository\n").expect("write .git");
    let bare = repo.dir().join("bare.git");
    repo.git(
        &["init", "-q", "--bare", bare.to_str().expect("utf-8 path")],
        None,
    );
    let unborn = Repo::new("head-unborn", &Script::default());
    let elsewhere = Repo::new("head-elsewhere", &script());
    let other = elsewhere.dir().join("other");
    fs::create_dir_all(&other).expect("other dir");
    elsewhere.git(
        &[
            "config",
            "core.worktree",
            other.to_str().expect("utf-8 path"),
        ],
        None,
    );

    let none: Vec<PathBuf> = vec![
        plain.0.clone(),
        repo.root.join("src"),
        repo.root.join(".git"),
        bare,
        unborn.root.clone(),
        elsewhere.root.clone(),
        fake.0.clone(),
    ];
    for root in &none {
        assert_eq!(head(root), Ok(None), "{}", root.display());
        assert_eq!(
            mine(root, &Window::default(), &all),
            Ok(None),
            "{}",
            root.display()
        );
    }
    assert!(head(&repo.root).expect("head").is_some());
    assert!(
        mine(&repo.root, &Window::default(), &all)
            .expect("mine")
            .is_some()
    );
}

#[test]
fn head_writes_nothing() {
    let repo = Repo::new("head-nowrite", &script());
    let before = snapshot(repo.dir());
    let first = head(&repo.root).expect("head");
    let second = head(&repo.root).expect("head");
    assert_eq!(snapshot(repo.dir()), before);
    assert_eq!(first, second);
}
