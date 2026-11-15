//! Query-time raw co-change counts (W14 G3a, A7): for one tracked path,
//! the kept non-merge commits each other tracked path shares with it. No
//! decay and no lift floor, the bulk skip kept and reported, and nothing
//! is stored or written.
use std::collections::{BTreeMap, BTreeSet};

use crate::history::{Commit, History};
use crate::score::{Aliases, BULK_FILES, touched};

/// One partner of the asked file: a current tracked path and the kept
/// commits it shared with the asked file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountPartner {
    pub path: String,
    pub count: u64,
}

/// The raw counts of one file's partners, sorted by count then path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountScan {
    /// HEAD at the read, hex.
    pub head: String,
    /// How many commits the window walked, merge commits included.
    pub commits_seen: u64,
    /// How many commits were skipped as bulk (over the kept-path cap).
    pub skipped_bulk: u64,
    /// Every partner with a shared kept commit, most shared first.
    pub partners: Vec<CountPartner>,
}

/// Every partner of `asked` over `history`: one count per kept,
/// non-bulk commit that touches the asked file.
pub(crate) fn count(history: &History, asked: &str) -> CountScan {
    let mut aliases = Aliases::default();
    let mut partners: BTreeMap<String, u64> = BTreeMap::new();
    let mut skipped_bulk = 0;
    for commit in &history.commits {
        let Some(touched) = kept(commit, &mut aliases, &mut skipped_bulk) else {
            continue;
        };
        if touched.contains(asked) {
            count_partners(&touched, asked, &mut partners);
        }
    }
    let mut partners: Vec<CountPartner> = partners
        .into_iter()
        .map(|(path, count)| CountPartner { path, count })
        .collect();
    partners.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.path.cmp(&b.path)));
    CountScan {
        head: history.head.clone(),
        commits_seen: history.commits.len() as u64,
        skipped_bulk,
        partners,
    }
}

/// The current names a commit counts, or none when it is a merge, touches
/// nothing, or is bulk (which `skipped_bulk` reports).
fn kept(
    commit: &Commit,
    aliases: &mut Aliases,
    skipped_bulk: &mut u64,
) -> Option<BTreeSet<String>> {
    let (touched, _) = touched(commit, aliases);
    if commit.merge {
        return None;
    }
    if touched.len() > BULK_FILES {
        *skipped_bulk += 1;
        return None;
    }
    Some(touched)
}

/// Adds one to every other tracked path in `touched`.
fn count_partners(touched: &BTreeSet<String>, asked: &str, partners: &mut BTreeMap<String, u64>) {
    for path in touched.iter().filter(|path| path.as_str() != asked) {
        *partners.entry(path.clone()).or_default() += 1;
    }
}
