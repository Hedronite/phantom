//! The workspace fingerprint of one index pass (SPEC-w5 G0b): what the
//! pass saw of the manifests and of the `.rs` files it could not read. A
//! stored fingerprint that differs rebuilds the graph, so a `Cargo.toml`
//! edit alone, or a change in which `.rs` files are skipped, is seen.

use crate::IndexError;
use crate::read::{Source, read_source};
use crate::walk::{Candidate, Walk};

/// Why the bounded read gave no text for a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unread {
    /// The read skipped it, or the walk did.
    Skipped,
    /// Its bytes are not UTF-8.
    NotUtf8,
}

impl Unread {
    fn tag(self) -> u8 {
        match self {
            Self::Skipped => b's',
            Self::NotUtf8 => b'u',
        }
    }
}

/// A file through the bounded read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Contents {
    Text(String),
    Unread(Unread),
}

impl Contents {
    pub(crate) fn read(file: &Candidate) -> Result<Self, IndexError> {
        Ok(match read_source(&file.abs_path)? {
            Source::Bytes { bytes, .. } => match String::from_utf8(bytes) {
                Ok(text) => Self::Text(text),
                Err(_) => Self::Unread(Unread::NotUtf8),
            },
            Source::Skipped(_) => Self::Unread(Unread::Skipped),
        })
    }
}

/// One walked `Cargo.toml` and what the bounded read found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestRead {
    pub path: String,
    pub contents: Contents,
}

/// Every walked `Cargo.toml` through the bounded read, in walk order.
pub(crate) fn read_manifests(walk: &Walk) -> Result<Vec<ManifestRead>, IndexError> {
    walk.files
        .iter()
        .filter(|file| file.path == "Cargo.toml" || file.path.ends_with("/Cargo.toml"))
        .map(|file| {
            Ok(ManifestRead {
                path: file.path.clone(),
                contents: Contents::read(file)?,
            })
        })
        .collect()
}

/// What `rebuild` stores after the extractor version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub fingerprint: [u8; 32],
    /// The number of `.rs` records.
    pub rs_skipped: u64,
}

/// The blake3 of the records, in this order:
///
/// 1. For each manifest, in walk order: `m`, its path, a NUL byte, then the
///    32-byte blake3 of its text, or `s` when skipped, or `u` when not UTF-8.
/// 2. For each skipped `.rs` file, then each one that is not UTF-8, in the
///    order given: `r`, its path, a NUL byte, then `s` or `u`.
pub(crate) fn stamp(manifests: &[ManifestRead], rust: &[(&str, Unread)]) -> Stamp {
    let mut hasher = blake3::Hasher::new();
    for manifest in manifests {
        record(&mut hasher, b'm', &manifest.path);
        match &manifest.contents {
            Contents::Text(text) => hasher.update(blake3::hash(text.as_bytes()).as_bytes()),
            Contents::Unread(unread) => hasher.update(&[unread.tag()]),
        };
    }
    for (path, unread) in rust {
        record(&mut hasher, b'r', path);
        hasher.update(&[unread.tag()]);
    }
    Stamp {
        fingerprint: *hasher.finalize().as_bytes(),
        rs_skipped: rust.len() as u64,
    }
}

fn record(hasher: &mut blake3::Hasher, kind: u8, path: &str) {
    hasher.update(&[kind]);
    hasher.update(path.as_bytes());
    hasher.update(&[0]);
}
