//! Walk a Rust tree, hash each file, and persist its extract (canon §6.4
//! steps 2, 3, and 5).
//!
//! The walk is language-agnostic; [`index`] keeps the `.rs` files and writes
//! each through [`Store::put_source`]. The walked tree is only read.
#![forbid(unsafe_code)]

mod read;
mod walk;

use std::fmt;
use std::path::Path;

use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{Store, StoreError};

pub use read::{Source, read_source};
pub use walk::{Candidate, MAX_FILE_BYTES, SkipReason, Skipped, Walk, is_secret, walk};

/// Version of the extraction this walker writes. Bump it when the stored
/// rows for an unchanged file would differ. The store does not record it in
/// this slice, so a bump needs a fresh database.
pub const EXTRACTOR_VERSION: u32 = 1;

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

/// Walks `root` and stores every kept `.rs` file whose hash is not already
/// stored for its path.
pub fn index(
    root: &Path,
    store: &mut Store,
    language: &RustLanguage,
) -> Result<IndexReport, IndexError> {
    let walk = walk(root)?;
    let mut report = IndexReport {
        skipped: walk.skipped,
        ..IndexReport::default()
    };
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
        if is_unchanged(store, &file.path, &source)? {
            report.unchanged.push(file.path.clone());
            continue;
        }
        store.put_source(&file.path, &source, mtime_ns, language)?;
        report.written.push(file.path.clone());
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
