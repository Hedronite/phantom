//! The freshness probe (SPEC-w9 G0b): what an [`index`](crate::index)
//! pass would change, found without writing anything.
//!
//! The probe walks with the indexer's own [`walk`] and reads with its own
//! [`read_source`], so a path the indexer skips is never reported. A kept
//! `.rs` file whose stored size and mtime both match is taken as
//! unchanged without being read; any other is read and hashed, and only a
//! hash that differs counts. The workspace fingerprint is recomputed the
//! way the indexer computes it, HEAD is compared with the HEAD the last
//! pass stored, and the stored extractor version with this binary's
//! (SPEC-w10 D8).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use phantom_fractal_store::{FileStat, Store};

use crate::fingerprint::{Unread, read_manifests, stamp};
use crate::read::{Source, mtime_ns, read_source};
use crate::walk::{Candidate, walk};
use crate::{EXTRACTOR_VERSION, IndexError, is_rust};

/// What [`probe`] found. Each list is sorted repo-relative paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Freshness {
    /// `.rs` files an index pass would write that are not stored.
    pub added: Vec<String>,
    /// Stored `.rs` files whose bytes are no longer the stored bytes.
    pub modified: Vec<String>,
    /// Stored paths an index pass would remove: gone, now skipped, or no
    /// longer UTF-8.
    pub removed: Vec<String>,
    /// The workspace fingerprint (manifests and unread `.rs` files) is not
    /// the stored one.
    pub fingerprint_changed: bool,
    /// The stored extractor version is not [`EXTRACTOR_VERSION`], so the
    /// next pass re-extracts every file.
    pub extractor_changed: bool,
    /// HEAD's commit oid now, or none when the root is not a git root.
    pub head: Option<String>,
    /// HEAD as the last index pass stored it.
    pub index_head: Option<String>,
}

impl Freshness {
    /// True when an index pass would change the index: a list is not
    /// empty, the fingerprint or the extractor version changed, or HEAD is
    /// not the stored HEAD.
    pub fn stale(&self) -> bool {
        !self.added.is_empty()
            || !self.modified.is_empty()
            || !self.removed.is_empty()
            || self.fingerprint_changed
            || self.extractor_changed
            || self.head != self.index_head
    }
}

/// Compares `root` with what `store` holds for it, reading only.
///
/// It fails as [`index`](crate::index) fails: when the root cannot be
/// walked, or a file it reads cannot be opened or read. A repository that
/// opens as a root but whose HEAD cannot be read fails it too.
pub fn probe(root: &Path, store: &Store) -> Result<Freshness, IndexError> {
    let walk = walk(root)?;
    let stored: BTreeMap<String, FileStat> = store
        .file_stats()?
        .into_iter()
        .map(|stat| (stat.path.clone(), stat))
        .collect();
    let mut out = Freshness::default();
    let mut kept: BTreeSet<&str> = BTreeSet::new();
    let mut read_skipped: Vec<&str> = Vec::new();
    let mut not_utf8: Vec<&str> = Vec::new();
    for file in walk.files.iter().filter(|file| is_rust(&file.path)) {
        let stat = stored.get(&file.path);
        if let Some(stat) = stat
            && same_stat(file, stat)?
        {
            kept.insert(&file.path);
            continue;
        }
        let bytes = match read_source(&file.abs_path)? {
            Source::Bytes { bytes, .. } => bytes,
            Source::Skipped(_) => {
                read_skipped.push(&file.path);
                continue;
            }
        };
        if std::str::from_utf8(&bytes).is_err() {
            not_utf8.push(&file.path);
            continue;
        }
        kept.insert(&file.path);
        match stat {
            None => out.added.push(file.path.clone()),
            Some(stat) if stat.blake3 != blake3::hash(&bytes).as_bytes() => {
                out.modified.push(file.path.clone());
            }
            Some(_) => {}
        }
    }
    out.removed = stored
        .keys()
        .filter(|path| !kept.contains(path.as_str()))
        .cloned()
        .collect();
    out.added.sort();
    out.modified.sort();

    // The indexer's order: the walk's `.rs` skips, the read's, then the
    // ones that are not UTF-8, each in walk order.
    let unread: Vec<(&str, Unread)> = walk
        .skipped
        .iter()
        .filter(|skip| is_rust(&skip.path))
        .map(|skip| (skip.path.as_str(), Unread::Skipped))
        .chain(read_skipped.into_iter().map(|path| (path, Unread::Skipped)))
        .chain(not_utf8.into_iter().map(|path| (path, Unread::NotUtf8)))
        .collect();
    let manifests = read_manifests(&walk)?;
    let fingerprint = stamp(&manifests, &unread).fingerprint;
    out.fingerprint_changed = store.workspace_fingerprint()? != Some(fingerprint);
    out.extractor_changed = store.extractor_version()? != Some(EXTRACTOR_VERSION);
    out.head = phantom_fractal_git::head(root)?;
    out.index_head = store.index_head()?;
    Ok(out)
}

/// True when the file's size and mtime on disk are the stored ones. The
/// mtime is read as [`read_source`] reads it, without following a link.
fn same_stat(file: &Candidate, stat: &FileStat) -> Result<bool, IndexError> {
    if i64::try_from(file.size).ok() != Some(stat.size) {
        return Ok(false);
    }
    let meta = fs::symlink_metadata(&file.abs_path)?;
    Ok(mtime_ns(&meta) == stat.mtime_ns)
}
