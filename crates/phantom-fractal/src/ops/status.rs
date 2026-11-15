//! `status`: how one index stands against its tree, and why (SPEC-w9
//! G0c). It reads the index, the tree and the lock's state, and writes
//! nothing; the lock file is never created.

use std::path::{Path, PathBuf};

use phantom_fractal_extract::{Freshness, probe};
use phantom_fractal_proto::{ErrorKind, SCHEMA_VERSION};
use phantom_fractal_store::{LockState, Store, index_database_path, lock_state};
use serde::Serialize;

use super::index::{CochangeSummary, stored_cochange};
use super::{Failure, Reply, canonical_repo, fresh, open_index};

/// What `status` is asked for.
pub(crate) struct StatusRequest {
    /// The indexed tree.
    pub(crate) repo: PathBuf,
}

/// `data` of `status`, with its keys in this order. It names the index by
/// its registry key and the tree by its directory name, never by a path.
#[derive(Serialize)]
pub(crate) struct StatusResult {
    /// The index's directory under the data directory's `repos`.
    key: String,
    name: String,
    head: Option<String>,
    index_head: Option<String>,
    stale: bool,
    changes: Changes,
    fingerprint_changed: bool,
    /// True when the index was written by another extractor version.
    extractor_changed: bool,
    counts: Counts,
    versions: Versions,
    epochs: Epochs,
    cochange: Option<CochangeSummary>,
    /// `held` while a writer holds the index, else `free`.
    lock: &'static str,
}

/// How many files an index pass would add, rewrite and remove.
#[derive(Serialize)]
struct Changes {
    added: usize,
    modified: usize,
    removed: usize,
}

/// The stored rows: files, symbols, and edges of every kind.
#[derive(Serialize)]
struct Counts {
    files: i64,
    symbols: i64,
    edges: i64,
}

/// The stored schema and extractor versions, and the envelope schema.
#[derive(Serialize)]
struct Versions {
    schema: i64,
    extractor: Option<u32>,
    envelope: u32,
}

#[derive(Serialize)]
struct Epochs {
    graph: u64,
    rank: u64,
}

/// The status of the index of `request.repo`. No index is `NoIndex`.
pub(crate) fn status(request: &StatusRequest) -> Result<Reply<StatusResult>, Failure> {
    let store = open_index(&request.repo)?;
    let root = canonical_repo(&request.repo)?;
    let database = index_database_path(&root);
    let freshness = probe_tree(&root, &store)?;
    let rank = store.rank_epoch()?;
    let result = result(&store, &database, &freshness, rank)?;
    Ok(Reply::with_meta(result, fresh::of(&freshness, rank)))
}

/// The tree at `root` compared with its index. The probe's error may hold
/// a path, so it is not passed on.
fn probe_tree(root: &Path, store: &Store) -> Result<Freshness, Failure> {
    probe(root, store).map_err(|_| {
        Failure::new(
            ErrorKind::Usage,
            "the tree could not be read to compare it with the index",
        )
    })
}

/// The reply: the index's key and name, the tree's state against it, and
/// what it stores, read in that order.
fn result(
    store: &Store,
    database: &Path,
    freshness: &Freshness,
    rank: u64,
) -> Result<StatusResult, Failure> {
    let key = key(database);
    let name = name(store, &key)?;
    let stored = stored(store, database, rank)?;
    Ok(StatusResult {
        name,
        key,
        head: freshness.head.clone(),
        index_head: freshness.index_head.clone(),
        stale: freshness.stale(),
        changes: Changes {
            added: freshness.added.len(),
            modified: freshness.modified.len(),
            removed: freshness.removed.len(),
        },
        fingerprint_changed: freshness.fingerprint_changed,
        extractor_changed: freshness.extractor_changed,
        counts: stored.counts,
        versions: stored.versions,
        epochs: stored.epochs,
        cochange: stored.cochange,
        lock: stored.lock,
    })
}

/// What the index stores, and whether a writer holds it.
struct Stored {
    counts: Counts,
    versions: Versions,
    epochs: Epochs,
    cochange: Option<CochangeSummary>,
    lock: &'static str,
}

/// The stored counts, versions, epochs and co-change scan of the index at
/// `database`, read in that order, then its lock.
fn stored(store: &Store, database: &Path, rank: u64) -> Result<Stored, Failure> {
    Ok(Stored {
        counts: counts(store)?,
        versions: versions(store)?,
        epochs: Epochs {
            graph: store.graph_epoch()?,
            rank,
        },
        cochange: stored_cochange(store)?,
        lock: lock_text(database)?,
    })
}

fn counts(store: &Store) -> Result<Counts, Failure> {
    Ok(Counts {
        files: store.row_count("files")?,
        symbols: store.row_count("symbols")?,
        edges: store.row_count("edges")?,
    })
}

fn versions(store: &Store) -> Result<Versions, Failure> {
    Ok(Versions {
        schema: store.schema_version()?,
        extractor: store.extractor_version()?,
        envelope: SCHEMA_VERSION,
    })
}

/// `held` while a writer holds the index at `database`, else `free`.
fn lock_text(database: &Path) -> Result<&'static str, Failure> {
    Ok(match lock_state(database)? {
        LockState::Held => "held",
        LockState::Free | LockState::Absent => "free",
    })
}

/// The registry key: the name of the database's directory.
fn key(database: &Path) -> String {
    database
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The tree's directory name, from the stored root, or the key without its
/// hash when no root is stored or the root has no name.
pub(crate) fn name(store: &Store, key: &str) -> Result<String, Failure> {
    let stored = store.root()?;
    let base = stored.as_deref().map(Path::new).and_then(Path::file_name);
    Ok(match base {
        Some(base) => base.to_string_lossy().into_owned(),
        None => key
            .rsplit_once('-')
            .map_or(key, |(slug, _)| slug)
            .to_owned(),
    })
}
