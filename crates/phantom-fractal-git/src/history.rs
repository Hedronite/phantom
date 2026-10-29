//! The all-parents walk and the per-commit tree diffs, through gix.
//!
//! Every gix call Phantom makes is in this file, and each is on the G1a
//! read-only allowlist (the G1a review, item 3):
//!
//! - A1 `gix::open_opts(root, Options::isolated())`
//! - A2 `Repository::workdir`, `Repository::object_hash`
//! - A3 `Repository::head` (`Head::is_unborn`), `Repository::head_id`
//! - A4 `Repository::find_commit`, `Commit::time`, `Commit::parent_ids`
//!   (over all parents, per the D4/D5 amendment),
//!   `Commit::tree_id`
//! - A5 `Repository::find_tree`, `TreeRefIter::from_bytes`
//! - A6 `gix::diff::tree` with `State` and `Recorder`
//! - A7 `Repository::find_blob`

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::path::Path;

use gix::ObjectId;
use gix::diff::tree::recorder::Change as Record;
use gix::diff::tree::{Recorder, State};
use gix::objs::TreeRefIter;

use crate::rename::{self, Side};
use crate::{GitError, Window};

/// Seconds in a day.
pub(crate) const DAY: i64 = 86_400;

/// What changed in one non-merge commit against its parent, in the commit's
/// own path names. Only blobs (regular or executable files) appear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Added(String),
    Deleted(String),
    Modified(String),
    Renamed { from: String, to: String },
}

/// One commit of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub oid: String,
    /// Committer time, unix seconds.
    pub time: i64,
    /// More than one parent: not diffed, so `changes` is empty, and it
    /// adds no weight.
    pub merge: bool,
    pub changes: Vec<Change>,
}

/// The window's commits in walk order: newest first by committer time,
/// ties by oid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct History {
    pub head: String,
    /// HEAD's committer time, unix seconds.
    pub head_time: i64,
    pub commits: Vec<Commit>,
}

/// The repository at `root`, opened isolated, and its HEAD commit, or
/// `None` when `root` is not the top of a git work tree or HEAD is
/// unborn. [`read`] and [`crate::head`] both open through here, so they
/// answer `None` for exactly the same roots.
pub(crate) fn open(root: &Path) -> Result<Option<(gix::Repository, ObjectId)>, GitError> {
    // Never search upward: the root itself must hold `.git`.
    if !root.join(".git").exists() {
        return Ok(None);
    }
    let Ok(repo) = gix::open_opts(root, gix::open::Options::isolated()) else {
        return Ok(None);
    };
    let Some(workdir) = repo.workdir() else {
        return Ok(None);
    };
    let same = match (workdir.canonicalize(), root.canonicalize()) {
        (Ok(workdir), Ok(root)) => workdir == root,
        _ => false,
    };
    if !same {
        return Ok(None);
    }
    if repo.head().map_err(GitError::new)?.is_unborn() {
        return Ok(None);
    }
    let head = repo.head_id().map_err(GitError::new)?.detach();
    Ok(Some((repo, head)))
}

/// The window of `root`'s history, or `None` when `root` is not the top
/// of a git work tree or HEAD is unborn.
pub(crate) fn read(root: &Path, window: &Window) -> Result<Option<History>, GitError> {
    let Some((repo, head)) = open(root)? else {
        return Ok(None);
    };
    let head_time = repo
        .find_commit(head)
        .map_err(GitError::new)?
        .time()
        .map_err(GitError::new)?
        .seconds;
    let floor = head_time.saturating_sub(i64::from(window.days) * DAY);

    // The walk (the D4/D5 amendment): every commit reachable from HEAD
    // through all parents, each once, newest first by committer time, ties
    // by the smaller oid.
    let mut queue: BinaryHeap<(i64, Reverse<ObjectId>)> = BinaryHeap::new();
    let mut queued: BTreeSet<ObjectId> = BTreeSet::new();
    queue.push((head_time, Reverse(head)));
    queued.insert(head);
    let mut state = State::default();
    let mut commits = Vec::new();
    while let Some((time, Reverse(id))) = queue.pop() {
        if commits.len() >= window.max_commits as usize || time < floor {
            break;
        }
        let commit = repo.find_commit(id).map_err(GitError::new)?;
        let parents: Vec<ObjectId> = commit.parent_ids().map(|id| id.detach()).collect();
        for parent in &parents {
            // A parent missing from the object store (a shallow boundary)
            // is not walked.
            if queued.insert(*parent)
                && let Ok(found) = repo.find_commit(*parent)
            {
                queue.push((
                    found.time().map_err(GitError::new)?.seconds,
                    Reverse(*parent),
                ));
            }
        }
        let merge = parents.len() > 1;
        let changes = match parents.first() {
            // A merge is not diffed; its parents are walked instead.
            _ if merge => Vec::new(),
            Some(parent) => match repo.find_commit(*parent) {
                Ok(parent) => diff(&repo, &mut state, &commit, Some(&parent))?,
                // The parent is not in the object store: seen, not diffed.
                Err(_) => Vec::new(),
            },
            None => diff(&repo, &mut state, &commit, None)?,
        };
        commits.push(Commit {
            oid: id.to_string(),
            time,
            merge,
            changes,
        });
    }
    Ok(Some(History {
        head: head.to_string(),
        head_time,
        commits,
    }))
}

/// The changes of `commit` against `parent`, or against the empty tree
/// for a root commit.
fn diff(
    repo: &gix::Repository,
    state: &mut State,
    commit: &gix::Commit<'_>,
    parent: Option<&gix::Commit<'_>>,
) -> Result<Vec<Change>, GitError> {
    let kind = repo.object_hash();
    let tree = repo
        .find_tree(commit.tree_id().map_err(GitError::new)?)
        .map_err(GitError::new)?;
    let base = match parent {
        Some(parent) => Some(
            repo.find_tree(parent.tree_id().map_err(GitError::new)?)
                .map_err(GitError::new)?,
        ),
        None => None,
    };
    let lhs = TreeRefIter::from_bytes(base.as_ref().map_or(&[][..], |t| &t.data), kind);
    let rhs = TreeRefIter::from_bytes(&tree.data, kind);
    let mut recorder = Recorder::default();
    gix::diff::tree(lhs, rhs, state, &repo.objects, &mut recorder).map_err(GitError::new)?;
    Ok(changes(recorder.records, |oid| {
        repo.find_blob(*oid)
            .ok()
            .map(|mut blob| std::mem::take(&mut blob.data))
    }))
}

/// The blob changes of one diff, with deletions and additions paired
/// into renames. A path that is not UTF-8 cannot be indexed and is left
/// out.
fn changes(records: Vec<Record>, read: impl FnMut(&ObjectId) -> Option<Vec<u8>>) -> Vec<Change> {
    let mut added: Vec<Side<ObjectId>> = Vec::new();
    let mut deleted: Vec<Side<ObjectId>> = Vec::new();
    let mut modified = Vec::new();
    for record in records {
        match record {
            Record::Addition {
                entry_mode,
                oid,
                path,
                ..
            } if entry_mode.is_blob() => push(&mut added, path, oid),
            Record::Deletion {
                entry_mode,
                oid,
                path,
                ..
            } if entry_mode.is_blob() => push(&mut deleted, path, oid),
            Record::Modification {
                previous_entry_mode,
                previous_oid,
                entry_mode,
                oid,
                path,
            } => match (previous_entry_mode.is_blob(), entry_mode.is_blob()) {
                (true, true) => {
                    if let Ok(path) = String::from_utf8(path.into()) {
                        modified.push(path);
                    }
                }
                (false, true) => push(&mut added, path, oid),
                (true, false) => push(&mut deleted, path, previous_oid),
                (false, false) => {}
            },
            _ => {}
        }
    }
    let pairs = rename::pair(&deleted, &added, read);
    let mut renamed_from = vec![false; deleted.len()];
    let mut renamed_to = vec![false; added.len()];
    let mut out: Vec<Change> = modified.into_iter().map(Change::Modified).collect();
    for (from, to) in pairs {
        renamed_from[from] = true;
        renamed_to[to] = true;
        out.push(Change::Renamed {
            from: deleted[from].path.clone(),
            to: added[to].path.clone(),
        });
    }
    for (side, renamed) in added.into_iter().zip(renamed_to) {
        if !renamed {
            out.push(Change::Added(side.path));
        }
    }
    for (side, renamed) in deleted.into_iter().zip(renamed_from) {
        if !renamed {
            out.push(Change::Deleted(side.path));
        }
    }
    out
}

fn push(sides: &mut Vec<Side<ObjectId>>, path: gix::bstr::BString, oid: ObjectId) {
    if let Ok(path) = String::from_utf8(path.into()) {
        sides.push(Side { path, oid });
    }
}
