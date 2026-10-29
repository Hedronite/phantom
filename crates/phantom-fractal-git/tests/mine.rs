//! `mine` on synthetic repositories (SPEC-w8 G1c): counts, decay, lift,
//! renames, bulk and merge skips, the window, the top-20 cut, the `None`
//! cases, determinism, and no write to the tree or `.git`.

mod support;

use std::fs;

use phantom_fractal_git::{Scan, Window, decay, mine};
use support::{DAY, HEAD, Op, Repo, Script, TempDir, indexed, snapshot};

/// `n` lines of 10 bytes each, numbered from `start`.
fn lines(start: u32, n: u32) -> String {
    (start..start + n)
        .map(|i| format!("line {i:04}\n"))
        .collect()
}

fn scan(repo: &Repo, window: &Window, paths: &[&str]) -> Scan {
    mine(&repo.root, window, &indexed(paths))
        .expect("mine")
        .expect("a git root")
}

/// Five commits at three ages: a and b together three times.
fn counts_script() -> Script {
    let mut script = Script::default();
    script.commit(
        "main",
        HEAD - 180 * DAY,
        &[Op::Write("a.rs", "1\n"), Op::Write("b.rs", "1\n")],
    );
    script.commit(
        "main",
        HEAD - 90 * DAY,
        &[Op::Write("a.rs", "2\n"), Op::Write("b.rs", "2\n")],
    );
    script.commit("main", HEAD - 90 * DAY, &[Op::Write("src/c.rs", "1\n")]);
    script.commit(
        "main",
        HEAD,
        &[
            Op::Write("a.rs", "3\n"),
            Op::Write("b.rs", "3\n"),
            Op::Write("README.md", "not indexed\n"),
        ],
    );
    script.commit("main", HEAD, &[Op::Write("d.rs", "1\n")]);
    script
}

const COUNTS_PATHS: [&str; 4] = ["a.rs", "b.rs", "src/c.rs", "d.rs"];

#[test]
fn counts_decay_and_lift_match_a_hand_count() {
    let repo = Repo::new("counts", &counts_script());
    let scan = scan(&repo, &Window::default(), &COUNTS_PATHS);
    assert_eq!(scan.last_oid.as_deref(), Some(repo.rev("main").as_str()));
    assert_eq!(
        scan.oldest_oid.as_deref(),
        Some(repo.rev("main~4").as_str())
    );
    assert_eq!((scan.commits_seen, scan.skipped_bulk), (5, 0));
    assert_eq!(scan.window_json, r#"{"days":365,"max_commits":5000}"#);

    // Decay 0.25 at 180 days, 0.5 at 90, 1 at 0.
    let files: Vec<(&str, f64, i64)> = scan
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.commits, file.last_time))
        .collect();
    assert_eq!(
        files,
        vec![
            ("a.rs", 1.75, HEAD),
            ("b.rs", 1.75, HEAD),
            ("d.rs", 1.0, HEAD),
            ("src/c.rs", 0.5, HEAD - 90 * DAY),
        ]
    );
    assert!(scan.files.iter().all(|file| file.renamed_from.is_none()));

    // N = 3.25; together = 0.25 + 0.5 + 1, one pair per commit of two.
    assert_eq!(scan.pairs.len(), 1);
    let pair = &scan.pairs[0];
    assert_eq!((pair.a.as_str(), pair.b.as_str()), ("a.rs", "b.rs"));
    assert_eq!((pair.together, pair.n_a, pair.n_b), (1.75, 1.75, 1.75));
    assert_eq!(pair.lift, 1.75 * 3.25 / (1.75 * 1.75));
    assert_eq!((pair.conf_ab, pair.conf_ba), (1.0, 1.0));
    assert_eq!(pair.last_oid, repo.rev("main~1"));
    assert_eq!(pair.last_time, HEAD);
}

#[test]
fn renames_carry_history_to_the_current_path() {
    let mut script = Script::default();
    let old = lines(0, 20);
    let new = format!("{}{}", lines(0, 14), lines(100, 6));
    script.commit(
        "main",
        HEAD,
        &[
            Op::Write("a.rs", "1\n"),
            Op::Write("b.rs", "1\n"),
            Op::Write("m.rs", &old),
        ],
    );
    script.commit(
        "main",
        HEAD,
        &[Op::Write("a.rs", "2\n"), Op::Write("b.rs", "2\n")],
    );
    script.commit(
        "main",
        HEAD,
        &[Op::Write("a.rs", "3\n"), Op::Write("b.rs", "3\n")],
    );
    // An identical-content rename and a 70%-similar rename in one commit.
    script.commit(
        "main",
        HEAD,
        &[
            Op::Rename("a.rs", "z.rs"),
            Op::Delete("m.rs"),
            Op::Write("n.rs", &new),
        ],
    );
    script.commit("main", HEAD, &[Op::Write("c.rs", "1\n")]);
    script.commit("main", HEAD, &[Op::Write("c.rs", "2\n")]);
    let repo = Repo::new("rename", &script);
    let scan = scan(&repo, &Window::default(), &["b.rs", "c.rs", "n.rs", "z.rs"]);

    let files: Vec<(&str, f64, Option<&str>)> = scan
        .files
        .iter()
        .map(|file| {
            (
                file.path.as_str(),
                file.commits,
                file.renamed_from.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        files,
        vec![
            ("b.rs", 3.0, None),
            ("c.rs", 2.0, None),
            ("n.rs", 2.0, Some("m.rs")),
            ("z.rs", 4.0, Some("a.rs")),
        ]
    );
    // b and z: three commits, the first with three kept files (weight 1/2).
    // N = 6, so lift = 2.5 * 6 / (3 * 4). n and z share only two commits.
    assert_eq!(scan.pairs.len(), 1);
    let pair = &scan.pairs[0];
    assert_eq!((pair.a.as_str(), pair.b.as_str()), ("b.rs", "z.rs"));
    assert_eq!((pair.together, pair.n_a, pair.n_b), (2.5, 3.0, 4.0));
    assert_eq!(pair.lift, 1.25);
}

#[test]
fn a_bulk_commit_is_skipped_and_counted_and_thirty_files_is_not_bulk() {
    let names: Vec<String> = (0..31).map(|n| format!("f{n:02}.rs")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut script = Script::default();
    let all: Vec<Op> = paths.iter().map(|path| Op::Write(path, "1\n")).collect();
    script.commit("main", HEAD, &all);
    let thirty: Vec<Op> = paths[..30]
        .iter()
        .map(|path| Op::Write(path, "2\n"))
        .collect();
    script.commit("main", HEAD, &thirty);
    let repo = Repo::new("bulk", &script);
    let scan = scan(&repo, &Window::default(), &paths);
    assert_eq!((scan.commits_seen, scan.skipped_bulk), (2, 1));
    assert_eq!(scan.files.len(), 30);
    assert!(scan.files.iter().all(|file| file.commits == 1.0));
    assert!(!scan.files.iter().any(|file| file.path == "f30.rs"));
}

#[test]
fn merges_are_skipped_and_only_the_first_parent_chain_is_walked() {
    let mut script = Script::default();
    let first = script.commit("main", HEAD - 2 * DAY, &[Op::Write("a.rs", "1\n")]);
    let side = script.branch("side", first, HEAD - DAY, &[Op::Write("s.rs", "1\n")]);
    script.commit("main", HEAD - DAY, &[Op::Write("a.rs", "2\n")]);
    script.merge("main", side, HEAD, &[Op::Write("s.rs", "1\n")]);
    let repo = Repo::new("merge", &script);
    let scan = scan(&repo, &Window::default(), &["a.rs", "s.rs"]);
    // The merge and the two main commits; the side commit is not walked.
    assert_eq!(scan.commits_seen, 3);
    assert_eq!(
        scan.oldest_oid.as_deref(),
        Some(repo.rev("main~2").as_str())
    );
    let files: Vec<&str> = scan.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(files, vec!["a.rs"]);
}

#[test]
fn the_window_cuts_by_days_inclusively_and_by_count() {
    let mut script = Script::default();
    for days_back in [20, 10, 0] {
        script.commit(
            "main",
            HEAD - days_back * DAY,
            &[Op::Write("a.rs", &format!("{days_back}\n"))],
        );
    }
    let repo = Repo::new("window", &script);
    let window = |days, max_commits| Window { days, max_commits };
    let seen = |w: Window| {
        let scan = scan(&repo, &w, &["a.rs"]);
        (scan.commits_seen, scan.files[0].commits, scan.window_json)
    };
    assert_eq!(
        seen(window(15, 5_000)),
        (
            2,
            1.0 + decay(10.0),
            r#"{"days":15,"max_commits":5000}"#.to_owned()
        )
    );
    // A commit exactly `days` older than HEAD is in.
    assert_eq!(seen(window(10, 5_000)).0, 2);
    assert_eq!(seen(window(9, 5_000)).0, 1);
    assert_eq!(seen(window(365, 2)).0, 2);
    // Cut by days and by count at once: the count comes first.
    assert_eq!(seen(window(15, 1)).0, 1);
    assert_eq!(seen(window(365, 5_000)).0, 3);
    let scan = scan(&repo, &window(15, 5_000), &["a.rs"]);
    assert_eq!(
        scan.oldest_oid.as_deref(),
        Some(repo.rev("main~1").as_str())
    );
}

#[test]
fn each_file_keeps_its_top_twenty_partners_and_a_pair_stays_if_either_end_keeps_it() {
    // 22 files, every pair changed together in 3 two-file commits 900 days
    // back, then one recent commit of another file. All pairs tie, so each
    // file keeps its first 20 partners by path. Only f20/f21 is dropped by
    // both ends.
    let names: Vec<String> = (0..22).map(|n| format!("f{n:02}.rs")).collect();
    let mut script = Script::default();
    let old = HEAD - 900 * DAY;
    for i in 0..22 {
        for j in i + 1..22 {
            for k in 0..3 {
                let body = format!("{i} {j} {k}\n");
                script.commit(
                    "main",
                    old,
                    &[Op::Write(&names[i], &body), Op::Write(&names[j], &body)],
                );
            }
        }
    }
    script.commit("main", HEAD, &[Op::Write("x.rs", "1\n")]);
    let repo = Repo::new("top20", &script);
    let mut paths: Vec<&str> = names.iter().map(String::as_str).collect();
    paths.push("x.rs");
    let scan = scan(
        &repo,
        &Window {
            days: 1_000,
            max_commits: 5_000,
        },
        &paths,
    );
    assert_eq!(scan.commits_seen, 694);

    let weight = decay(900.0);
    assert_eq!(weight, 1.0 / 1024.0);
    let total = 1.0 + 693.0 * weight;
    let lift = 3.0 * weight * total / ((63.0 * weight) * (63.0 * weight));
    assert!(lift > 1.0);
    assert_eq!(scan.pairs.len(), 230);
    assert!(
        scan.pairs
            .iter()
            .all(|pair| pair.lift == lift && pair.together == 3.0 * weight)
    );
    let has = |a: &str, b: &str| scan.pairs.iter().any(|pair| pair.a == a && pair.b == b);
    assert!(!has("f20.rs", "f21.rs"));
    assert!(has("f00.rs", "f21.rs"));
    assert!(has("f19.rs", "f21.rs"));
    assert!(has("f20.rs", "f19.rs") || has("f19.rs", "f20.rs"));
}

#[test]
fn no_root_a_subdirectory_a_bare_repo_and_an_unborn_head_are_none() {
    let all = indexed(&["a.rs"]);
    let plain = TempDir::new("plain");
    assert_eq!(mine(&plain.0, &Window::default(), &all), Ok(None));

    let mut script = Script::default();
    script.commit(
        "main",
        HEAD,
        &[Op::Write("src/lib.rs", "1\n"), Op::Write("a.rs", "1\n")],
    );
    let repo = Repo::new("none", &script);
    assert!(
        mine(&repo.root, &Window::default(), &all)
            .expect("mine")
            .is_some()
    );
    // Never upward: a subdirectory of a work tree, or the git dir itself.
    assert_eq!(
        mine(&repo.root.join("src"), &Window::default(), &all),
        Ok(None)
    );
    assert_eq!(
        mine(&repo.root.join(".git"), &Window::default(), &all),
        Ok(None)
    );

    let bare = repo.dir().join("bare.git");
    repo.git(
        &["init", "-q", "--bare", bare.to_str().expect("utf-8 path")],
        None,
    );
    assert_eq!(mine(&bare, &Window::default(), &all), Ok(None));

    let unborn = Repo::new("unborn", &Script::default());
    assert_eq!(mine(&unborn.root, &Window::default(), &all), Ok(None));

    // A work tree configured elsewhere: the root is not its top.
    let elsewhere = Repo::new("elsewhere", &script);
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
    assert_eq!(mine(&elsewhere.root, &Window::default(), &all), Ok(None));

    let fake = TempDir::new("fake");
    fs::write(fake.0.join(".git"), "not a repository\n").expect("write .git");
    assert_eq!(mine(&fake.0, &Window::default(), &all), Ok(None));
}

#[test]
fn mine_writes_nothing_and_repeats_exactly() {
    let repo = Repo::new("nowrite", &counts_script());
    let before = snapshot(repo.dir());
    let first = scan(&repo, &Window::default(), &COUNTS_PATHS);
    let second = scan(&repo, &Window::default(), &COUNTS_PATHS);
    assert_eq!(snapshot(repo.dir()), before);
    assert_eq!(first, second);
    // An identical repository built again gives an equal scan.
    let again = Repo::new("nowrite-again", &counts_script());
    assert_eq!(scan(&again, &Window::default(), &COUNTS_PATHS), first);
}

#[test]
fn the_window_serializes_canonically_and_checks_its_ranges() {
    assert_eq!(
        Window::default(),
        Window {
            days: 365,
            max_commits: 5_000
        }
    );
    assert_eq!(
        Window::default().to_json(),
        r#"{"days":365,"max_commits":5000}"#
    );
    assert!(
        Window {
            days: 1,
            max_commits: 1
        }
        .in_range()
    );
    assert!(
        Window {
            days: 3_650,
            max_commits: 50_000
        }
        .in_range()
    );
    assert!(
        !Window {
            days: 0,
            max_commits: 1
        }
        .in_range()
    );
    assert!(
        !Window {
            days: 3_651,
            max_commits: 1
        }
        .in_range()
    );
    assert!(
        !Window {
            days: 1,
            max_commits: 0
        }
        .in_range()
    );
    assert!(
        !Window {
            days: 1,
            max_commits: 50_001
        }
        .in_range()
    );
}

#[test]
fn only_files_count_and_a_deleted_namesake_gives_no_history() {
    let mut script = Script::default();
    script.commit("main", HEAD, &[Op::Write("gone.rs", "1\n")]);
    script.commit("main", HEAD, &[Op::Write("gone.rs", "2\n")]);
    script.commit("main", HEAD, &[Op::Delete("gone.rs")]);
    script.commit(
        "main",
        HEAD,
        &[Op::Write("a.rs", "1\n"), Op::Link("link.rs", "a.rs")],
    );
    script.commit(
        "main",
        HEAD,
        &[Op::Write("a.rs", "2\n"), Op::Link("link.rs", "b.rs")],
    );
    let repo = Repo::new("kinds", &script);
    // gone.rs is indexed as an untracked file of the same name: the old,
    // deleted file's history is not its history. link.rs is a symlink.
    let scan = scan(&repo, &Window::default(), &["a.rs", "gone.rs", "link.rs"]);
    let files: Vec<(&str, f64)> = scan
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.commits))
        .collect();
    assert_eq!(files, vec![("a.rs", 2.0)]);
}
