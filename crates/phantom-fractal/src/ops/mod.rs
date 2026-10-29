//! The operations behind every command, shared by the command line and
//! the MCP server.
//!
//! Each operation is a plain function: a request struct in, its `data` or
//! a [`Failure`] out. None parses arguments or writes to stdout; the
//! caller turns the result into an envelope with [`envelope`]. Every path
//! an operation returns is relative to the indexed tree, and no message
//! holds an absolute path.

pub(crate) mod calls;
pub(crate) mod find;
pub(crate) mod impact;
pub(crate) mod index;
pub(crate) mod lookup;
pub(crate) mod show;
mod source;
pub(crate) mod version;

use std::path::Path;

use phantom_fractal_extract::IndexError;
use phantom_fractal_proto::{Envelope, ErrorKind};
use phantom_fractal_store::{Store, StoreError, index_database_path};
use serde::Serialize;

pub(crate) use calls::{CallsRequest, callees, callers};
pub(crate) use find::{FindRequest, find};
pub(crate) use impact::{ImpactRequest, impact};
pub(crate) use index::{IndexRequest, index};
pub(crate) use show::{ShowRequest, Target, show};
pub(crate) use version::{VersionRequest, version};

use crate::ops::lookup::Candidates;

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
    result: Result<T, Failure>,
) -> (serde_json::Result<String>, i32) {
    match result {
        Ok(data) => (serde_json::to_string(&Envelope::success(data)), 0),
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
fn open_index(repo: &Path) -> Result<Store, Failure> {
    let root = canonical_repo(repo)?;
    let database = index_database_path(&root);
    if !database.exists() {
        return Err(Failure::new(
            ErrorKind::NoIndex,
            "no index for this path; run index first",
        ));
    }
    Ok(Store::open(&database)?)
}

/// `repo` canonicalized, or `NotFound` when it does not exist.
fn canonical_repo(repo: &Path) -> Result<std::path::PathBuf, Failure> {
    repo.canonicalize()
        .map_err(|_| Failure::new(ErrorKind::NotFound, "repo path not found"))
}
