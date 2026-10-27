//! Walk a Rust tree, hash each file, and persist its extract (canon §6.4
//! steps 2, 3, and 5).
//!
//! The walk is language-agnostic; [`index`] keeps the `.rs` files and writes
//! each through [`Store::put_source`], then rebuilds the crate graph and the
//! call edges when anything changed (SPEC-w3 G2c). The walked tree is only
//! read; nothing in it is compiled or run.
#![forbid(unsafe_code)]

pub mod graph;
mod read;
mod rebuild;
mod walk;

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use phantom_fractal_lang::{Edge, Reason, ResolveCounts};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{Store, StoreError};

pub use read::{Source, read_source};
pub use rebuild::stored_graph;
pub use walk::{Candidate, MAX_FILE_BYTES, SkipReason, Skipped, Walk, is_secret, walk};

/// Version of the extraction this walker writes, stored after each graph
/// rebuild. Bump it when the stored rows for an unchanged file would differ:
/// a database stamped with any other version is fully re-extracted. Version
/// 2 adds call references, methods, scopes, and edges; version 3 adds the
/// `self-method` and `self-assoc` edges.
pub const EXTRACTOR_VERSION: u32 = 3;

/// What one [`index`] pass did, by repo-relative path, in walk order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Extracted and written this pass.
    pub written: Vec<String>,
    /// Already stored with the same hash; nothing written.
    pub unchanged: Vec<String>,
    /// `.rs` files the walk kept whose bytes are not UTF-8.
    pub not_utf8: Vec<String>,
    /// Everything the walk skipped, any extension.
    pub skipped: Vec<Skipped>,
    /// Stored `.rs` paths this pass no longer keeps; their rows are gone.
    pub removed: Vec<String>,
    /// The crate graph and resolve pass, or none when nothing changed and
    /// the stored extractor version is current, so nothing was rebuilt.
    pub graph: Option<GraphReport>,
}

/// What one crate-graph rebuild found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphReport {
    pub modules: usize,
    /// Kept `.rs` files no target reaches.
    pub orphans: usize,
    pub counts: ResolveCounts,
    /// The edges placed, by reason.
    pub edges_by_reason: ReasonCounts,
}

/// Edges placed for each of the seven reasons, zero included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReasonCounts {
    pub path_exact: u64,
    pub local: u64,
    pub use_alias: u64,
    pub assoc: u64,
    pub self_method: u64,
    pub self_assoc: u64,
    pub crate_path: u64,
}

impl ReasonCounts {
    /// Every reason in declaration order.
    pub const REASONS: [Reason; 7] = [
        Reason::PathExact,
        Reason::Local,
        Reason::UseAlias,
        Reason::Assoc,
        Reason::SelfMethod,
        Reason::SelfAssoc,
        Reason::CratePath,
    ];

    /// The count of each reason among `edges`.
    pub fn of(edges: &[Edge]) -> Self {
        let mut counts = Self::default();
        for edge in edges {
            *counts.slot(edge.reason) += 1;
        }
        counts
    }

    /// The count for `reason`.
    pub fn get(&self, reason: Reason) -> u64 {
        let mut copy = *self;
        *copy.slot(reason)
    }

    fn slot(&mut self, reason: Reason) -> &mut u64 {
        match reason {
            Reason::PathExact => &mut self.path_exact,
            Reason::Local => &mut self.local,
            Reason::UseAlias => &mut self.use_alias,
            Reason::Assoc => &mut self.assoc,
            Reason::SelfMethod => &mut self.self_method,
            Reason::SelfAssoc => &mut self.self_assoc,
            Reason::CratePath => &mut self.crate_path,
        }
    }
}

/// Why an [`index`] pass stopped.
#[derive(Debug)]
#[non_exhaustive]
pub enum IndexError {
    /// The root could not be walked, or a kept file could not be opened or
    /// read.
    Io(std::io::Error),
    /// The store refused a read or a write.
    Store(StoreError),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "walk failed: {err}"),
            Self::Store(err) => write!(f, "store failed: {err}"),
        }
    }
}

impl std::error::Error for IndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Store(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for IndexError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<StoreError> for IndexError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// Walks `root`, stores every kept `.rs` file whose hash is not already
/// stored for its path, and removes the stored paths the walk no longer
/// keeps. When anything was written or removed, or the stored extractor
/// version is not [`EXTRACTOR_VERSION`], it then rebuilds the crate graph,
/// resolves calls, replaces the edges, and stores the version. A stored
/// version other than the current one re-extracts every file first.
pub fn index(
    root: &Path,
    store: &mut Store,
    language: &RustLanguage,
) -> Result<IndexReport, IndexError> {
    let walk = walk(root)?;
    let reextract = store.extractor_version()? != Some(EXTRACTOR_VERSION);
    let mut report = IndexReport {
        skipped: walk.skipped.clone(),
        ..IndexReport::default()
    };
    let mut kept: Vec<&Candidate> = Vec::new();
    for file in walk.files.iter().filter(|file| is_rust(&file.path)) {
        let (bytes, mtime_ns) = match read_source(&file.abs_path)? {
            Source::Bytes { bytes, mtime_ns } => (bytes, mtime_ns),
            Source::Skipped(reason) => {
                report.skipped.push(Skipped {
                    path: file.path.clone(),
                    reason,
                });
                continue;
            }
        };
        let Ok(source) = String::from_utf8(bytes) else {
            report.not_utf8.push(file.path.clone());
            continue;
        };
        kept.push(file);
        if !reextract && is_unchanged(store, &file.path, &source)? {
            report.unchanged.push(file.path.clone());
            continue;
        }
        store.put_source(&file.path, &source, mtime_ns, language)?;
        report.written.push(file.path.clone());
    }
    let kept_paths: HashSet<&str> = kept.iter().map(|file| file.path.as_str()).collect();
    for path in store.file_paths()? {
        if !kept_paths.contains(path.as_str()) {
            store.remove_file(&path)?;
            report.removed.push(path);
        }
    }
    if reextract || !report.written.is_empty() || !report.removed.is_empty() {
        report.graph = Some(rebuild::rebuild(root, &walk, &kept, store, language)?);
    }
    Ok(report)
}

fn is_rust(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "rs")
}

/// True when the store already holds `path` with the blake3 of `source`.
fn is_unchanged(store: &Store, path: &str, source: &str) -> Result<bool, IndexError> {
    let hash = blake3::hash(source.as_bytes());
    Ok(store
        .stored_blake3(path)?
        .is_some_and(|stored| stored.as_slice() == hash.as_bytes()))
}
