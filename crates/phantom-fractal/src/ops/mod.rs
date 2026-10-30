//! The operations behind every command, shared by the command line and
//! the MCP server.
//!
//! Each operation is a plain function: a request struct in, its [`Reply`]
//! or a [`Failure`] out. None parses arguments or writes to stdout; the
//! caller turns the result into an envelope with [`envelope`]. Every path
//! an operation returns is relative to the indexed tree, and no message
//! holds an absolute path.

pub(crate) mod budget;
pub(crate) mod calls;
pub(crate) mod cochange;
pub(crate) mod diff;
pub(crate) mod doctor;
pub(crate) mod find;
pub(crate) mod fresh;
pub(crate) mod impact;
pub(crate) mod index;
pub(crate) mod lookup;
pub(crate) mod map;
pub(crate) mod path;
pub(crate) mod rank;
pub(crate) mod registry;
pub(crate) mod show;
mod source;
pub(crate) mod status;
pub(crate) mod version;

use std::path::Path;

use phantom_fractal_extract::IndexError;
use phantom_fractal_proto::{Envelope, ErrorKind, Meta};
use phantom_fractal_store::{Store, StoreError, index_database_path};
use serde::Serialize;

pub(crate) use calls::{CallsRequest, callees, callers};
pub(crate) use cochange::{CochangeRequest, cochange};
pub(crate) use diff::{DiffRequest, impact_diff};
pub(crate) use doctor::{DoctorRequest, doctor};
pub(crate) use find::{FindRequest, find};
pub(crate) use impact::{ImpactRequest, impact};
pub(crate) use index::{IndexRequest, index};
pub(crate) use map::{MapRequest, map};
pub(crate) use path::{PathRequest, symbol_path};
pub(crate) use rank::{RankKind, RankRequest, rank};
pub(crate) use show::{ShowRequest, Target, show};
pub(crate) use status::{StatusRequest, status};
pub(crate) use version::{VersionRequest, version};

use crate::ops::budget::Shed;
use crate::ops::lookup::Candidates;

/// A successful op's `data`, and its `meta` when the op read an index.
pub(crate) struct Reply<T> {
    data: T,
    meta: Option<Meta>,
}

impl<T> Reply<T> {
    /// `data` from an op that reads no index, so with no `meta`.
    pub(crate) fn plain(data: T) -> Self {
        Self { data, meta: None }
    }

    /// `data` with the `meta` of the index it came from.
    pub(crate) fn with_meta(data: T, meta: Meta) -> Self {
        Self {
            data,
            meta: Some(meta),
        }
    }
}

/// `data`, read from `store`, the index of `repo`, with its `meta`, and
/// fitted to `max_tokens` with `meta` counted like any other byte of the
/// line.
pub(crate) fn answer<T: Shed>(
    data: T,
    store: &Store,
    repo: &Path,
    max_tokens: Option<u64>,
) -> Result<Reply<T>, Failure> {
    let meta = fresh::meta(&canonical_repo(repo)?, store)?;
    let data = budget::fit(data, max_tokens, &meta)?;
    Ok(Reply::with_meta(data, meta))
}

/// An envelope error. `message` and `candidates` never hold an absolute
/// path.
pub(crate) struct Failure {
    kind: ErrorKind,
    message: String,
    /// The envelope's `data` for an `Ambiguous` lookup.
    candidates: Option<Candidates>,
}

impl Failure {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            candidates: None,
        }
    }

    /// `Ambiguous`, with the candidates as the envelope's `data`.
    fn ambiguous(message: impl Into<String>, candidates: Candidates) -> Self {
        Self {
            candidates: Some(candidates),
            ..Self::new(ErrorKind::Ambiguous, message)
        }
    }
}

impl From<StoreError> for Failure {
    fn from(err: StoreError) -> Self {
        let kind = match err {
            StoreError::SchemaNewer { .. } => ErrorKind::SchemaNewer,
            StoreError::LockHeld => ErrorKind::LockHeld,
            _ => ErrorKind::Usage,
        };
        Self::new(kind, format!("store: {err}"))
    }
}

impl From<IndexError> for Failure {
    fn from(err: IndexError) -> Self {
        match err {
            IndexError::Store(err) => err.into(),
            other => Self::new(ErrorKind::Usage, other.to_string()),
        }
    }
}

/// The envelope for `result` as one line of JSON without its newline, and
/// the exit code it carries: 0 on success, else its kind's code. The line
/// is the one the command line prints.
pub(crate) fn envelope<T: Serialize>(
    result: Result<Reply<T>, Failure>,
) -> (serde_json::Result<String>, i32) {
    match result {
        Ok(Reply { data, meta }) => {
            let envelope = match meta {
                Some(meta) => Envelope::success_with_meta(data, meta),
                None => Envelope::success(data),
            };
            (serde_json::to_string(&envelope), 0)
        }
        Err(Failure {
            kind,
            message,
            candidates,
        }) => {
            let envelope = match candidates {
                Some(candidates) => Envelope::failure_with_data(kind, message, candidates),
                None => Envelope::<Candidates>::failure(kind, message),
            };
            (serde_json::to_string(&envelope), kind.exit_code())
        }
    }
}

/// The store for the tree at `repo`, which must already be indexed. It is
/// `NotFound` when `repo` does not exist and `NoIndex` when it has no
/// database; neither case creates anything.
pub(crate) fn open_index(repo: &Path) -> Result<Store, Failure> {
    let root = canonical_repo(repo)?;
    if !has_index(&root) {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "no index for this path; run index first",
        ));
    }
    Ok(Store::open(&index_database_path(&root))?)
}

/// True when the canonical tree `root` has a database in the data
/// directory. Nothing is opened or created.
pub(crate) fn has_index(root: &Path) -> bool {
    index_database_path(root).exists()
}

/// `repo` canonicalized, or `NotFound` when it does not exist.
pub(crate) fn canonical_repo(repo: &Path) -> Result<std::path::PathBuf, Failure> {
    repo.canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "repo path not found"))
}
