//! Rename aliases, weights, filters and the top-20 cut (D5, canon §7.2).
//!
//! Commits are read newest first. Each path a commit names is mapped to
//! the file it is now through the renames seen so far, so older history
//! counts toward the current path. Merge commits and bulk commits still
//! move the aliases but add no weight.
//!
//! For a kept commit with `n` indexed files, `decay = 0.5^(age_days / 90)`
//! with `age_days` measured back from HEAD's committer time (a commit
//! newer than HEAD counts as age 0). Each file adds `decay` to its count,
//! each unordered pair adds `decay / (n - 1)` to `together`, and `N` is
//! the sum of `decay` over the kept commits. Then `conf_ab = together /
//! n_a`, `conf_ba = together / n_b` and `lift = together * N / (n_a *
//! n_b)`. A pair needs at least 3 commits together and `lift > 1`, and it
//! stays when either end ranks it in its top 20 by lift, then `together`,
//! then the partner's path.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use phantom_fractal_store::{FileRow, PairRow};

use crate::history::{Change, DAY, History};
use crate::{Scan, Window};

/// A commit with more kept files than this is bulk: skipped and counted.
pub const BULK_FILES: usize = 30;

/// The decay half-life.
pub const HALF_LIFE_DAYS: f64 = 90.0;

/// The fewest commits a pair must share.
pub const MIN_TOGETHER: u64 = 3;

/// The most partners each file keeps.
pub const TOP_PARTNERS: usize = 20;

/// `0.5^(age_days / 90)`, with a negative age counted as 0.
pub fn decay(age_days: f64) -> f64 {
    0.5_f64.powf(age_days.max(0.0) / HALF_LIFE_DAYS)
}

/// Historical names mapped to the file each is now. A name absent from
/// the map is its own current name; `None` means no current file.
#[derive(Default)]
struct Aliases(BTreeMap<String, Option<String>>);

impl Aliases {
    fn current(&self, path: &str) -> Option<String> {
        match self.0.get(path) {
            Some(current) => current.clone(),
            None => Some(path.to_owned()),
        }
    }
}

#[derive(Default)]
struct FileTally {
    commits: f64,
    last_time: Option<i64>,
}

#[derive(Default)]
struct PairTally {
    together: f64,
    count: u64,
    last: Option<(String, i64)>,
}

/// The scan of `history`, counting only `indexed` paths.
pub(crate) fn score(history: &History, window: &Window, indexed: &BTreeSet<String>) -> Scan {
    let mut aliases = Aliases::default();
    let mut renamed_from: BTreeMap<String, String> = BTreeMap::new();
    let mut files: BTreeMap<String, FileTally> = BTreeMap::new();
    let mut pairs: BTreeMap<(String, String), PairTally> = BTreeMap::new();
    let mut total = 0.0;
    let mut skipped_bulk = 0;

    for commit in &history.commits {
        // The files this commit changed, by their current names.
        let mut touched = BTreeSet::new();
        // Names that, before this commit, belonged to no current file,
        // and renames that carry a current file to an older name.
        let mut gone = Vec::new();
        let mut carried = Vec::new();
        for change in &commit.changes {
            match change {
                Change::Modified(path) => touched.extend(aliases.current(path)),
                Change::Added(path) => {
                    touched.extend(aliases.current(path));
                    gone.push(path.clone());
                }
                Change::Deleted(path) => gone.push(path.clone()),
                Change::Renamed { from, to } => {
                    let current = aliases.current(to);
                    if let Some(current) = &current {
                        touched.insert(current.clone());
                        renamed_from
                            .entry(current.clone())
                            .or_insert_with(|| from.clone());
                    }
                    gone.push(to.clone());
                    carried.push((from.clone(), current));
                }
            }
        }
        for path in gone {
            aliases.0.insert(path, None);
        }
        for (from, current) in carried {
            aliases.0.insert(from, current);
        }

        if commit.merge {
            continue;
        }
        let kept: Vec<String> = touched
            .into_iter()
            .filter(|path| indexed.contains(path))
            .collect();
        let n = kept.len();
        if n == 0 {
            continue;
        }
        if n > BULK_FILES {
            skipped_bulk += 1;
            continue;
        }
        let age_days = history.head_time.saturating_sub(commit.time) as f64 / DAY as f64;
        let weight = decay(age_days);
        total += weight;
        for path in &kept {
            let tally = files.entry(path.clone()).or_default();
            tally.commits += weight;
            tally.last_time.get_or_insert(commit.time);
        }
        if n < 2 {
            continue;
        }
        let share = weight / (n - 1) as f64;
        for (i, a) in kept.iter().enumerate() {
            for b in &kept[i + 1..] {
                let tally = pairs.entry((a.clone(), b.clone())).or_default();
                tally.together += share;
                tally.count += 1;
                tally
                    .last
                    .get_or_insert_with(|| (commit.oid.clone(), commit.time));
            }
        }
    }

    let count = |path: &str| files.get(path).map_or(0.0, |tally| tally.commits);
    let candidates: Vec<PairRow> = pairs
        .into_iter()
        .filter(|(_, tally)| tally.count >= MIN_TOGETHER)
        .filter_map(|((a, b), tally)| {
            let (n_a, n_b) = (count(&a), count(&b));
            let lift = tally.together * total / (n_a * n_b);
            let (last_oid, last_time) = tally.last?;
            (lift > 1.0).then(|| PairRow {
                together: tally.together,
                n_a,
                n_b,
                lift,
                conf_ab: tally.together / n_a,
                conf_ba: tally.together / n_b,
                a,
                b,
                last_oid,
                last_time,
            })
        })
        .collect();
    let keep = top_partners(&candidates);
    let kept: Vec<PairRow> = candidates
        .into_iter()
        .enumerate()
        .filter(|(position, _)| keep.contains(position))
        .map(|(_, pair)| pair)
        .collect();

    let oldest_oid = history.commits.last().map(|commit| commit.oid.clone());
    Scan {
        last_oid: Some(history.head.clone()),
        oldest_oid,
        commits_seen: history.commits.len() as u64,
        skipped_bulk,
        window_json: window.to_json(),
        files: files
            .into_iter()
            .map(|(path, tally)| FileRow {
                renamed_from: renamed_from.get(&path).cloned(),
                commits: tally.commits,
                last_time: tally.last_time.unwrap_or(history.head_time),
                path,
            })
            .collect(),
        pairs: kept,
    }
}

/// The positions in `pairs` that either end ranks in its top
/// [`TOP_PARTNERS`]: by lift, then `together`, both descending, then the
/// partner's path.
fn top_partners(pairs: &[PairRow]) -> BTreeSet<usize> {
    let mut by_file: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
    for (position, pair) in pairs.iter().enumerate() {
        by_file
            .entry(&pair.a)
            .or_default()
            .push((position, &pair.b));
        by_file
            .entry(&pair.b)
            .or_default()
            .push((position, &pair.a));
    }
    let mut keep = BTreeSet::new();
    for mut partners in by_file.into_values() {
        partners.sort_by(|(x, x_path), (y, y_path)| {
            descending(pairs[*x].lift, pairs[*y].lift)
                .then(descending(pairs[*x].together, pairs[*y].together))
                .then(x_path.cmp(y_path))
        });
        keep.extend(
            partners
                .iter()
                .take(TOP_PARTNERS)
                .map(|(position, _)| *position),
        );
    }
    keep
}

fn descending(a: f64, b: f64) -> Ordering {
    b.total_cmp(&a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Commit;

    const HEAD: i64 = 1_800_000_000;

    fn commit(oid: &str, days_back: i64, changes: Vec<Change>) -> Commit {
        Commit {
            oid: oid.to_owned(),
            time: HEAD - days_back * DAY,
            merge: false,
            changes,
        }
    }

    fn modified(paths: &[&str]) -> Vec<Change> {
        paths
            .iter()
            .map(|p| Change::Modified((*p).to_owned()))
            .collect()
    }

    fn indexed(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    fn history(commits: Vec<Commit>) -> History {
        History {
            head: "head".to_owned(),
            head_time: HEAD,
            commits,
        }
    }

    #[test]
    fn decay_halves_every_ninety_days_and_clamps_the_future() {
        assert_eq!(decay(0.0), 1.0);
        assert_eq!(decay(90.0), 0.5);
        assert_eq!(decay(180.0), 0.25);
        assert_eq!(decay(-5.0), 1.0);
    }

    #[test]
    fn a_rename_chain_carries_old_names_to_the_current_path() {
        // Newest first: b.rs -> c.rs, then a.rs -> b.rs, then edits of a.rs.
        let scan = score(
            &history(vec![
                commit(
                    "3",
                    0,
                    vec![Change::Renamed {
                        from: "b.rs".into(),
                        to: "c.rs".into(),
                    }],
                ),
                commit(
                    "2",
                    0,
                    vec![Change::Renamed {
                        from: "a.rs".into(),
                        to: "b.rs".into(),
                    }],
                ),
                commit("1", 0, modified(&["a.rs"])),
            ]),
            &Window::default(),
            &indexed(&["c.rs"]),
        );
        assert_eq!(scan.files.len(), 1);
        assert_eq!(scan.files[0].path, "c.rs");
        assert_eq!(scan.files[0].commits, 3.0);
        // The last hop.
        assert_eq!(scan.files[0].renamed_from.as_deref(), Some("b.rs"));
    }

    #[test]
    fn a_name_reused_after_a_delete_is_a_different_file() {
        // x.rs was deleted, then a new x.rs was added. The old file's edits
        // do not count toward the new one.
        let scan = score(
            &history(vec![
                commit("3", 0, vec![Change::Added("x.rs".into())]),
                commit("2", 0, vec![Change::Deleted("x.rs".into())]),
                commit("1", 0, modified(&["x.rs"])),
            ]),
            &Window::default(),
            &indexed(&["x.rs"]),
        );
        assert_eq!(scan.files[0].commits, 1.0);
    }

    #[test]
    fn merges_and_bulk_commits_add_no_weight_but_bulk_is_counted() {
        let many: Vec<String> = (0..31).map(|n| format!("f{n:02}.rs")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        let mut merge = commit("m", 0, modified(&["f00.rs", "f01.rs"]));
        merge.merge = true;
        let scan = score(
            &history(vec![
                merge,
                commit("bulk", 0, modified(&names)),
                commit("thirty", 0, modified(&names[..30])),
            ]),
            &Window::default(),
            &indexed(&names),
        );
        assert_eq!(scan.skipped_bulk, 1);
        assert_eq!(scan.commits_seen, 3);
        assert_eq!(scan.files.len(), 30);
        assert!(scan.files.iter().all(|file| file.commits == 1.0));
    }

    #[test]
    fn weights_lift_and_the_thresholds_follow_canon() {
        // a and b together in 3 commits, a alone in 1, c alone in 2.
        let mut commits = Vec::new();
        for n in 0..3 {
            commits.push(commit(&format!("ab{n}"), 0, modified(&["a.rs", "b.rs"])));
        }
        commits.push(commit("a", 0, modified(&["a.rs"])));
        commits.push(commit("c0", 0, modified(&["c.rs"])));
        commits.push(commit("c1", 0, modified(&["c.rs", "d.rs"])));
        let scan = score(
            &history(commits),
            &Window::default(),
            &indexed(&["a.rs", "b.rs", "c.rs", "d.rs"]),
        );
        // N = 6, n_a = 4, n_b = 3, together = 3 * 1 / (2 - 1).
        assert_eq!(scan.pairs.len(), 1);
        let pair = &scan.pairs[0];
        assert_eq!((pair.a.as_str(), pair.b.as_str()), ("a.rs", "b.rs"));
        assert_eq!((pair.together, pair.n_a, pair.n_b), (3.0, 4.0, 3.0));
        assert_eq!(pair.lift, 3.0 * 6.0 / 12.0);
        assert_eq!((pair.conf_ab, pair.conf_ba), (0.75, 1.0));
        // The newest commit the pair shares.
        assert_eq!((pair.last_oid.as_str(), pair.last_time), ("ab0", HEAD));
    }

    #[test]
    fn a_lift_of_exactly_one_or_two_shared_commits_is_dropped() {
        // e and f only ever change together: lift = 3 * 3 / (3 * 3) = 1.
        let commits = (0..3)
            .map(|n| commit(&format!("ef{n}"), 0, modified(&["e.rs", "f.rs"])))
            .collect();
        let scan = score(
            &history(commits),
            &Window::default(),
            &indexed(&["e.rs", "f.rs"]),
        );
        assert!(scan.pairs.is_empty());
        // Two shared commits with a high lift are still too few.
        let mut commits: Vec<Commit> = (0..2)
            .map(|n| commit(&format!("ef{n}"), 0, modified(&["e.rs", "f.rs"])))
            .collect();
        commits.extend((0..8).map(|n| commit(&format!("g{n}"), 0, modified(&["g.rs"]))));
        let scan = score(
            &history(commits),
            &Window::default(),
            &indexed(&["e.rs", "f.rs", "g.rs"]),
        );
        assert!(scan.pairs.is_empty());
    }
}
