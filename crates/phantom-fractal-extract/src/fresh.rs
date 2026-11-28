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

use phantom_fractal_store::StoreError;

use crate::fingerprint::{ManifestRead, Unread, read_manifests_with, stamp_with_extra};
use crate::read::{Source, mtime_ns, read_source};
use crate::registry::Registry;
use crate::walk::{Candidate, Walk, walk_with};
use crate::{EXTRACTOR_VERSION, IndexError, PrefilterState, build_driver_filters};

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
    let registry = crate::default_registry().map_err(StoreError::Extract)?;
    probe_with(root, store, &registry)
}

/// Compares `root` with what `store` holds using registered languages in `registry`.
pub fn probe_with(
    root: &Path,
    store: &Store,
    registry: &Registry<'_>,
) -> Result<Freshness, IndexError> {
    let walk = walk_with(root, registry)?;
    let manifests = read_manifests_with(&walk, registry)?;
    let prefilter = build_driver_filters(&walk, &manifests, registry)?;
    probe_walk(root, store, &walk, &manifests, registry, &prefilter)
}

fn probe_walk(
    root: &Path,
    store: &Store,
    walk: &Walk,
    manifests: &[ManifestRead],
    registry: &Registry<'_>,
    prefilter: &PrefilterState,
) -> Result<Freshness, IndexError> {
    let stored = stored_stats_map(store)?;
    let scan = scan_sources(&walk.files, &stored, &|path| {
        prefilter.keeps_source(registry, path)
    })?;
    let mut out = finish_scan_lists(scan.freshness, &stored, &scan.kept);
    let mut unread = collect_unread(walk, &scan.read_skipped, &scan.not_utf8, &|path| {
        registry.is_supported(path)
    });
    let owned_unread = crate::driver_unread_owned(prefilter, registry);
    unread.extend(owned_unread.iter().map(|(p, u)| (p.as_str(), *u)));
    let fingerprint = stamp_with_extra(manifests, &unread, &prefilter.excluded_hashes).fingerprint;
    fill_probe_meta(root, store, fingerprint, &mut out)?;
    Ok(out)
}

fn stored_stats_map(store: &Store) -> Result<BTreeMap<String, FileStat>, IndexError> {
    Ok(store
        .file_stats()?
        .into_iter()
        .map(|stat| (stat.path.clone(), stat))
        .collect())
}

fn finish_scan_lists(
    mut out: Freshness,
    stored: &BTreeMap<String, FileStat>,
    kept: &BTreeSet<&str>,
) -> Freshness {
    out.removed = stored
        .keys()
        .filter(|path| !kept.contains(path.as_str()))
        .cloned()
        .collect();
    out.added.sort();
    out.modified.sort();
    out
}

fn fill_probe_meta(
    root: &Path,
    store: &Store,
    fingerprint: [u8; 32],
    out: &mut Freshness,
) -> Result<(), IndexError> {
    out.fingerprint_changed = store.workspace_fingerprint()? != Some(fingerprint);
    out.extractor_changed = store.extractor_version()? != Some(EXTRACTOR_VERSION);
    out.head = phantom_fractal_git::head(root)?;
    out.index_head = store.index_head()?;
    Ok(())
}

struct ScanState<'a> {
    freshness: Freshness,
    kept: BTreeSet<&'a str>,
    read_skipped: Vec<&'a str>,
    not_utf8: Vec<&'a str>,
}

fn scan_sources<'a, F>(
    files: &'a [Candidate],
    stored: &BTreeMap<String, FileStat>,
    supported: &F,
) -> Result<ScanState<'a>, IndexError>
where
    F: Fn(&str) -> bool,
{
    let mut state = ScanState {
        freshness: Freshness::default(),
        kept: BTreeSet::new(),
        read_skipped: Vec::new(),
        not_utf8: Vec::new(),
    };
    for file in files.iter().filter(|file| supported(&file.path)) {
        classify_source(file, stored.get(&file.path), &mut state)?;
    }
    Ok(state)
}

fn classify_source<'a>(
    file: &'a Candidate,
    stat: Option<&FileStat>,
    state: &mut ScanState<'a>,
) -> Result<(), IndexError> {
    if let Some(stat) = stat
        && same_stat(file, stat)?
    {
        state.kept.insert(&file.path);
        return Ok(());
    }
    let Some(bytes) = read_utf8_bytes(file, state)? else {
        return Ok(());
    };
    state.kept.insert(&file.path);
    record_bytes_change(&file.path, &bytes, stat, &mut state.freshness);
    Ok(())
}

fn read_utf8_bytes<'a>(
    file: &'a Candidate,
    state: &mut ScanState<'a>,
) -> Result<Option<Vec<u8>>, IndexError> {
    let Source::Bytes { bytes, .. } = read_source(&file.abs_path)? else {
        state.read_skipped.push(&file.path);
        return Ok(None);
    };
    if std::str::from_utf8(&bytes).is_err() {
        state.not_utf8.push(&file.path);
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn record_bytes_change(path: &str, bytes: &[u8], stat: Option<&FileStat>, out: &mut Freshness) {
    match stat {
        None => out.added.push(path.to_owned()),
        Some(stat) if stat.blake3 != blake3::hash(bytes).as_bytes() => {
            out.modified.push(path.to_owned());
        }
        Some(_) => {}
    }
}

fn collect_unread<'a, F>(
    walk: &'a Walk,
    read_skipped: &[&'a str],
    not_utf8: &[&'a str],
    supported: &F,
) -> Vec<(&'a str, Unread)>
where
    F: Fn(&str) -> bool,
{
    walk.skipped
        .iter()
        .filter(|skip| supported(&skip.path))
        .map(|skip| (skip.path.as_str(), Unread::Skipped))
        .chain(read_skipped.iter().map(|path| (*path, Unread::Skipped)))
        .chain(not_utf8.iter().map(|path| (*path, Unread::NotUtf8)))
        .collect()
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
