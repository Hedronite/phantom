//! Query-time raw co-change counts (W14 G3a, A7): tracked paths from the
//! in-process history walk, no decay, no lift floor, the bulk skip kept
//! and reported, sorted by count then path, deterministic, and no write.
mod support;

use phantom_fractal_git::{CountScan, GitError, Window, cochange_counts};
use support::{DAY, HEAD, Op, Repo, Script, TempDir, snapshot};

fn counts(repo: &Repo, path: &str) -> CountScan {
    cochange_counts(&repo.root, &Window::default(), path)
        .expect("count")
        .expect("a git root")
}

fn pairs(scan: &CountScan) -> Vec<(&str, u64)> {
    scan.partners
        .iter()
        .map(|partner| (partner.path.as_str(), partner.count))
        .collect()
}

#[test]
fn counts_are_raw_cover_every_tracked_path_and_sort_by_count_then_path() {
    let mut script = Script::default();
    // Three hundred days old and still counted whole: no decay.
    script.commit(
        "main",
        HEAD - 300 * DAY,
        &[Op::Write("a.rs", "1\n"), Op::Write("b.rs", "1\n")],
    );
    // A non-Rust tracked path counts too, beside the asked file.
    script.commit(
        "main",
        HEAD,
        &[
            Op::Write("a.rs", "2\n"),
            Op::Write("b.rs", "2\n"),
            Op::Write("README.md", "docs\n"),
        ],
    );
    script.commit("main", HEAD, &[Op::Write("src/c.rs", "1\n")]);
    script.commit(
        "main",
        HEAD,
        &[Op::Write("b.rs", "3\n"), Op::Write("README.md", "docs\n")],
    );
    let repo = Repo::new("raw-counts", &script);
    let scan = counts(&repo, "a.rs");
    assert_eq!(scan.head, repo.rev("main"));
    assert_eq!(scan.commits_seen, 4);
    assert_eq!(scan.skipped_bulk, 0);
    // Two shared commits for b, one for the README, sorted by count then
    // path: a single shared commit is enough, so there is no floor.
    assert_eq!(pairs(&scan), vec![("b.rs", 2), ("README.md", 1)]);
}

#[test]
fn a_rename_carries_older_history_to_the_current_path() {
    let mut script = Script::default();
    script.commit(
        "main",
        HEAD - 2 * DAY,
        &[Op::Write("old.rs", "1\n"), Op::Write("peer.rs", "1\n")],
    );
    script.commit("main", HEAD - DAY, &[Op::Rename("old.rs", "new.rs")]);
    script.commit(
        "main",
        HEAD,
        &[Op::Write("new.rs", "2\n"), Op::Write("peer.rs", "2\n")],
    );
    let repo = Repo::new("raw-rename", &script);
    assert_eq!(pairs(&counts(&repo, "new.rs")), vec![("peer.rs", 2)]);
    assert_eq!(pairs(&counts(&repo, "peer.rs")), vec![("new.rs", 2)]);
}

#[test]
fn a_commit_at_the_cap_counts_and_one_over_it_is_bulk() {
    let mut script = Script::default();
    // Exactly the cap: counted whole.
    let names: Vec<String> = (0..29).map(|n| format!("at{n:02}.txt")).collect();
    let mut ops: Vec<Op> = names
        .iter()
        .map(|name| Op::Write(name.as_str(), "x\n"))
        .collect();
    ops.push(Op::Write("a.rs", "1\n"));
    script.commit("main", HEAD - DAY, &ops);
    // One over the cap: skipped and reported, so only the first counts.
    let over: Vec<String> = (0..30).map(|n| format!("over{n:02}.txt")).collect();
    let mut ops: Vec<Op> = over
        .iter()
        .map(|name| Op::Write(name.as_str(), "x\n"))
        .collect();
    ops.push(Op::Write("a.rs", "2\n"));
    script.commit("main", HEAD, &ops);
    let repo = Repo::new("raw-cap", &script);
    let scan = counts(&repo, "a.rs");
    assert_eq!(scan.skipped_bulk, 1, "{scan:?}");
    assert_eq!(scan.partners.len(), 29, "{scan:?}");
    assert!(scan.partners.iter().all(|partner| partner.count == 1));
    assert_eq!(scan.partners[0].path, "at00.txt");
}

#[test]
fn merge_commits_add_nothing() {
    let mut script = Script::default();
    let base = script.commit(
        "main",
        HEAD - 3 * DAY,
        &[Op::Write("a.rs", "1\n"), Op::Write("b.rs", "1\n")],
    );
    let side = script.branch("side", base, HEAD - 2 * DAY, &[Op::Write("b.rs", "2\n")]);
    script.merge(
        "main",
        side,
        HEAD,
        &[Op::Write("a.rs", "2\n"), Op::Write("b.rs", "3\n")],
    );
    let repo = Repo::new("raw-merge", &script);
    let scan = counts(&repo, "a.rs");
    assert_eq!(pairs(&scan), vec![("b.rs", 1)]);
    assert_eq!(scan.commits_seen, 3);
}

#[test]
fn the_count_is_deterministic_and_writes_nothing() {
    let mut script = Script::default();
    script.commit(
        "main",
        HEAD - DAY,
        &[Op::Write("a.rs", "1\n"), Op::Write("b.rs", "1\n")],
    );
    script.commit("main", HEAD, &[Op::Write("a.rs", "2\n")]);
    let repo = Repo::new("raw-steady", &script);
    let before = snapshot(&repo.root);
    let one = counts(&repo, "a.rs");
    let two = counts(&repo, "a.rs");
    assert_eq!(one, two);
    assert_eq!(before, snapshot(&repo.root));
}

#[test]
fn a_directory_that_is_not_a_repository_answers_none() {
    let dir = TempDir::new("raw-none");
    assert!(
        cochange_counts(&dir.0, &Window::default(), "a.rs")
            .expect("no error")
            .is_none()
    );
}

#[test]
fn a_file_deleted_at_head_is_not_tracked_and_says_so() {
    let mut script = Script::default();
    script.commit(
        "main",
        HEAD - DAY,
        &[Op::Write("gone.rs", "1\n"), Op::Write("a.rs", "1\n")],
    );
    script.commit("main", HEAD, &[Op::Delete("gone.rs")]);
    let repo = Repo::new("raw-deleted", &script);
    match cochange_counts(&repo.root, &Window::default(), "gone.rs") {
        Err(GitError::NotTracked { path, head }) => {
            assert_eq!(path, "gone.rs");
            assert_eq!(head, repo.rev("main"));
        }
        other => panic!("a deleted file must not count: {other:?}"),
    }
}

#[test]
fn a_path_that_never_existed_is_not_tracked() {
    let mut script = Script::default();
    script.commit("main", HEAD, &[Op::Write("a.rs", "1\n")]);
    let repo = Repo::new("raw-never", &script);
    match cochange_counts(&repo.root, &Window::default(), "src/never.rs") {
        Err(GitError::NotTracked { path, head }) => {
            assert_eq!(path, "src/never.rs");
            assert_eq!(head, repo.rev("main"));
        }
        other => panic!("an absent path must not count: {other:?}"),
    }
}
