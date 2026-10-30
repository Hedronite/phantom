//! The work tree against a base commit (SPEC-w9 G2a, D7): which files an
//! index of the work tree would add, change or lose next to the base.
//!
//! Only objects, trees and references are read. The git index
//! (`.git/index`) is never opened, no git filter runs, and renames are not
//! paired: a moved file is one removal and one addition. A work-tree file
//! is compared by its raw bytes, through their blob id, with the base
//! tree's blob.
//!
//! The gix calls added for G2a, beside the W8 allowlist in [`history`]:
//!
//! - B1 `Repository::try_find_reference` (`refs/heads/<name>`,
//!   `refs/tags/<name>`)
//! - B2 `Reference::peel_to_commit`
//! - B3 the object store's `lookup_prefix`, with `Prefix::from_hex`
//! - B4 `Repository::find_object`, `Object::peel_to_commit`
//! - B5 `gix::objs::compute_hash` (a work-tree file's blob id)
//!
//! and, from the W8 list, A1 to A3 (the same open as [`crate::head`]), A4
//! `find_commit` and `tree_id`, A5 `find_tree` and `TreeRefIter`, A6
//! `gix::diff::tree` (against the empty tree, to list the base tree) and
//! A7 `find_blob`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use gix::ObjectId;
use gix::diff::tree::recorder::Change as Record;
use gix::diff::tree::{Recorder, State};
use gix::objs::TreeRefIter;

use crate::GitError;
use crate::history;

/// The shortest hex oid prefix a base may be given as.
pub const MIN_OID_HEX: usize = 7;

/// What the work tree is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Base {
    /// The commit HEAD points at.
    Head,
    /// A branch name, a tag name, or a hex oid of at least
    /// [`MIN_OID_HEX`] digits that names one commit. A name that is both
    /// a branch and a tag must name one commit through both.
    Ref(String),
}

/// How one file changed from the base to the work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileChange {
    /// Kept in the work tree, not in the base.
    Added,
    /// In both, with other bytes.
    Modified,
    /// In the base, not kept in the work tree.
    Removed,
}

impl FileChange {
    /// The stable lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    /// Repo-relative, `/`-separated.
    pub path: String,
    pub change: FileChange,
    /// The base blob's bytes, for a modified or removed file.
    pub base: Option<Vec<u8>>,
}

/// What [`changes`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The base commit, 40 hex.
    pub base_oid: String,
    /// Every changed file, sorted by path.
    pub files: Vec<Changed>,
}

/// Why [`changes`] gave no answer for a git root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesError {
    /// The base names no commit: not a branch, a tag, or a known oid
    /// prefix of at least [`MIN_OID_HEX`] hex digits.
    UnknownBase,
    /// The base names more than one commit: an oid prefix of several
    /// objects, or a branch and a tag of that name on other commits.
    AmbiguousBase,
    /// The repository could not be read.
    Git(GitError),
}

impl fmt::Display for ChangesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBase => write!(
                f,
                "the base is not a branch, a tag, or a commit id of at least {MIN_OID_HEX} hex digits"
            ),
            Self::AmbiguousBase => write!(f, "the base names more than one commit"),
            Self::Git(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ChangesError {}

impl From<GitError> for ChangesError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

/// The files that differ between `base` and the work tree at `root`.
///
/// `worktree` holds every file an index of the work tree keeps, by
/// repo-relative path, with its bytes. A base blob counts only when
/// `keep_base(path, size)` says an index would have kept it there; a base
/// path the work tree does not keep, and `keep_base` refuses, is not a
/// change. A path that is not UTF-8, a symlink and a submodule are never
/// counted.
///
/// `Ok(None)` for exactly the roots [`crate::head`] answers `None` for.
pub fn changes(
    root: &Path,
    base: &Base,
    worktree: &BTreeMap<String, Vec<u8>>,
    keep_base: &dyn Fn(&str, u64) -> bool,
) -> Result<Option<Changes>, ChangesError> {
    let Some((repo, head)) = history::open(root)? else {
        return Ok(None);
    };
    let base_oid = match base {
        Base::Head => head,
        Base::Ref(name) => resolve(&repo, name)?,
    };
    let kind = repo.object_hash();
    let tree = repo
        .find_tree(
            repo.find_commit(base_oid)
                .map_err(GitError::new)?
                .tree_id()
                .map_err(GitError::new)?,
        )
        .map_err(GitError::new)?;
    let mut recorder = Recorder::default();
    gix::diff::tree(
        TreeRefIter::from_bytes(&[], kind),
        TreeRefIter::from_bytes(&tree.data, kind),
        &mut State::default(),
        &repo.objects,
        &mut recorder,
    )
    .map_err(GitError::new)?;
    let mut base_blobs: BTreeMap<String, ObjectId> = BTreeMap::new();
    for record in recorder.records {
        if let Record::Addition {
            entry_mode,
            oid,
            path,
            ..
        } = record
            && entry_mode.is_blob()
            && let Ok(path) = String::from_utf8(path.into())
        {
            base_blobs.insert(path, oid);
        }
    }

    let read = |oid: ObjectId| -> Result<Vec<u8>, GitError> {
        Ok(std::mem::take(
            &mut repo.find_blob(oid).map_err(GitError::new)?.data,
        ))
    };
    let mut files = Vec::new();
    for (path, oid) in &base_blobs {
        let here = worktree.get(path);
        if let Some(bytes) = here
            && gix::objs::compute_hash(kind, gix::objs::Kind::Blob, bytes).map_err(GitError::new)?
                == *oid
        {
            continue;
        }
        let bytes = read(*oid)?;
        let kept = keep_base(path, bytes.len() as u64);
        let change = match (here.is_some(), kept) {
            (true, true) => FileChange::Modified,
            (true, false) => FileChange::Added,
            (false, true) => FileChange::Removed,
            (false, false) => continue,
        };
        files.push(Changed {
            path: path.clone(),
            change,
            base: kept.then_some(bytes),
        });
    }
    for path in worktree.keys() {
        if !base_blobs.contains_key(path) {
            files.push(Changed {
                path: path.clone(),
                change: FileChange::Added,
                base: None,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(Changes {
        base_oid: base_oid.to_string(),
        files,
    }))
}

/// The commit `name` gives: a branch, a tag (peeled to its commit), or
/// else a hex oid prefix of at least [`MIN_OID_HEX`] digits.
fn resolve(repo: &gix::Repository, name: &str) -> Result<ObjectId, ChangesError> {
    let mut found: Option<ObjectId> = None;
    for full in [format!("refs/heads/{name}"), format!("refs/tags/{name}")] {
        // A name the reference rules refuse names no reference.
        let Ok(Some(mut reference)) = repo.try_find_reference(full.as_str()) else {
            continue;
        };
        let commit = reference
            .peel_to_commit()
            .map_err(|_| ChangesError::UnknownBase)?
            .id;
        match found {
            Some(other) if other != commit => return Err(ChangesError::AmbiguousBase),
            _ => found = Some(commit),
        }
    }
    if let Some(commit) = found {
        return Ok(commit);
    }
    let hex_len = repo.object_hash().len_in_hex();
    if !(MIN_OID_HEX..=hex_len).contains(&name.len())
        || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ChangesError::UnknownBase);
    }
    let prefix = gix::hash::Prefix::from_hex(&name.to_ascii_lowercase())
        .map_err(|_| ChangesError::UnknownBase)?;
    let oid = match repo.objects.lookup_prefix(prefix, None) {
        Ok(Some(Ok(oid))) => oid,
        Ok(Some(Err(()))) => return Err(ChangesError::AmbiguousBase),
        Ok(None) => return Err(ChangesError::UnknownBase),
        Err(err) => return Err(GitError::new(err).into()),
    };
    repo.find_object(oid)
        .map_err(GitError::new)?
        .peel_to_commit()
        .map(|commit| commit.id)
        .map_err(|_| ChangesError::UnknownBase)
}
