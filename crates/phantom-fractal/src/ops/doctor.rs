//! `doctor`: checks the install and the data directory (SPEC-w9 G3b,
//! D11), in a fixed order, and repairs nothing.
//!
//! It names a directory by its role or its key, never by its path. A
//! database's schema is read from its SQLite header, and an index is read
//! only through the store's guarded read-only open, which creates and
//! changes nothing; an index in use is reported not checked. It always
//! succeeds: `ok` carries the verdict.

use std::fs;
use std::path::{Path, PathBuf};

use phantom_fractal_store::{LockState, SCHEMA_VERSION, Store, StoreError, lock_state};
use serde::Serialize;

use super::registry::{header_schema, indexes, repos_dir};
use super::{Failure, Reply};

/// The tree-sitter-rust version `build.rs` read from `Cargo.lock`, or
/// empty.
const GRAMMAR: &str = env!("PHANTOM_FRACTAL_BUILD_GRAMMAR");

/// `data` of `doctor`.
#[derive(Serialize)]
pub(crate) struct DoctorResult {
    /// True when every check passed.
    ok: bool,
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

/// What `doctor` is asked for: nothing.
pub(crate) struct DoctorRequest;

/// The checks, in their fixed order. It never fails.
pub(crate) fn doctor(_request: &DoctorRequest) -> Result<Reply<DoctorResult>, Failure> {
    let repos = repos_dir();
    let mut keys = indexes();
    keys.sort();
    let databases: Vec<(String, PathBuf)> = keys
        .iter()
        .map(|key| {
            let dir = repos.clone().unwrap_or_default().join(key);
            (key.clone(), dir.join("graph.db"))
        })
        .collect();
    let checks = vec![
        data_dir(repos.as_deref(), &keys),
        database_check(&databases),
        locks(&databases),
        Check {
            name: "extractor",
            ok: true,
            detail: phantom_fractal_extract::EXTRACTOR_VERSION.to_string(),
        },
        Check {
            name: "grammar",
            ok: true,
            detail: if GRAMMAR.is_empty() {
                "tree-sitter-rust, version unknown".to_owned()
            } else {
                format!("tree-sitter-rust {GRAMMAR}")
            },
        },
        Check {
            name: "network",
            ok: true,
            detail: "none: Phantom has no network code".to_owned(),
        },
        git(&databases),
    ];
    Ok(Reply::plain(DoctorResult {
        ok: checks.iter().all(|check| check.ok),
        checks,
    }))
}

/// The permission bits of `path`, without following a link.
fn mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        fs::symlink_metadata(path).ok().map(|_| 0)
    }
}

fn owner(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path).ok().map(|meta| meta.uid())
    }
    #[cfg(not(unix))]
    {
        fs::symlink_metadata(path).ok().map(|_| 0)
    }
}

/// True when `path` is a real directory that only its owner can open.
fn private_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
        && mode(path).is_some_and(|mode| mode & 0o077 == 0)
}

/// The data directory, `repos/`, each index directory and each database
/// file: `repos/` and the index directories private real directories, the
/// data directory not writable by group or others, and every one owned by
/// the data directory's owner. Only what the tree itself shows is read:
/// no process identity, which would need a write or `libc`.
fn data_dir(repos: Option<&Path>, keys: &[String]) -> Check {
    let check = |ok, detail: String| Check {
        name: "data_dir",
        ok,
        detail,
    };
    let Some(repos) = repos else {
        return check(false, "the data directory could not be named".to_owned());
    };
    let data = repos.parent().unwrap_or(repos);
    if !data.exists() {
        return check(true, "no data directory yet; index makes it".to_owned());
    }
    let mut problems = Vec::new();
    let data_mode = mode(data).unwrap_or(0);
    if data_mode & 0o022 != 0 {
        problems.push(format!(
            "the data directory is {data_mode:04o}, writable by others"
        ));
    }
    let mut members = vec![data.to_path_buf()];
    if repos.exists() {
        if !private_dir(repos) {
            problems.push(format!(
                "repos/ is {:04o}, not a private directory",
                mode(repos).unwrap_or(0)
            ));
        }
        members.push(repos.to_path_buf());
    }
    for key in keys {
        let dir = repos.join(key);
        if !private_dir(&dir) {
            problems.push(format!(
                "{key} is {:04o}, not 0700",
                mode(&dir).unwrap_or(0)
            ));
        }
        for file in ["graph.db", "graph.db-wal", "graph.db-shm"] {
            let file = dir.join(file);
            if fs::symlink_metadata(&file).is_ok() {
                members.push(file);
            }
        }
        members.push(dir);
    }
    let data_owner = owner(data);
    if members.iter().any(|path| owner(path) != data_owner) {
        problems.push(
            "its directories and databases are not all owned by the data directory's owner"
                .to_owned(),
        );
    }
    if problems.is_empty() {
        check(
            true,
            format!(
                "the data directory, repos/ and {} index directories are private, with one owner",
                keys.len()
            ),
        )
    } else {
        check(false, problems.join("; "))
    }
}

/// Each `graph.db`, and its WAL and shared memory when present, is
/// `0600`, and its schema is not newer than this binary's.
fn database_check(databases: &[(String, PathBuf)]) -> Check {
    let mut ok = true;
    let mut details = Vec::new();
    for (key, database) in databases {
        let mut notes = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", database.display()));
            let Some(mode) = mode(&file) else {
                continue;
            };
            if mode != 0o600 {
                ok = false;
                notes.push(format!("graph.db{suffix} is {mode:04o}, not 0600"));
            }
        }
        match header_schema(database) {
            None => {
                ok = false;
                notes.push("not an SQLite database".to_owned());
            }
            Some(found) if found > SCHEMA_VERSION => {
                ok = false;
                notes.push(format!(
                    "schema {found}, newer than this binary's {SCHEMA_VERSION}"
                ));
            }
            Some(found) if found < SCHEMA_VERSION => {
                notes.push(format!(
                    "schema {found}, which index migrates to {SCHEMA_VERSION}"
                ));
            }
            Some(found) => notes.push(format!("schema {found}")),
        }
        details.push(format!("{key}: {}", notes.join(", ")));
    }
    Check {
        name: "databases",
        ok,
        detail: if details.is_empty() {
            "no index yet".to_owned()
        } else {
            details.join("; ")
        },
    }
}

/// Whether a writer holds each index. A held lock is not a fault.
fn locks(databases: &[(String, PathBuf)]) -> Check {
    let mut ok = true;
    let details: Vec<String> = databases
        .iter()
        .map(|(key, database)| match lock_state(database) {
            Ok(LockState::Held) => format!("{key}: held"),
            Ok(LockState::Free | LockState::Absent) => format!("{key}: free"),
            Err(_) => {
                ok = false;
                format!("{key}: unreadable")
            }
        })
        .collect();
    Check {
        name: "locks",
        ok,
        detail: if details.is_empty() {
            "no index yet".to_owned()
        } else {
            details.join("; ")
        },
    }
}

/// For each index the guarded read-only open reads, whether HEAD reads
/// at its stored tree. Only a HEAD that cannot be read is a fault.
fn git(databases: &[(String, PathBuf)]) -> Check {
    let mut ok = true;
    let in_use = |key: &str| format!("{key}: not checked, the index is in use");
    let details: Vec<String> = databases
        .iter()
        .map(|(key, database)| {
            let store = match Store::open_read_only(database) {
                Ok(store) => store,
                Err(StoreError::ReadOnlyUnavailable) => return in_use(key),
                Err(StoreError::SchemaNewer { .. } | StoreError::SchemaMismatch { .. }) => {
                    return format!("{key}: not checked, its schema is not this binary's");
                }
                Err(_) => return format!("{key}: not checked, it cannot be read"),
            };
            let root = store.root();
            // A writer that started during the read makes it unreliable.
            if !store.read_only_still_valid().unwrap_or(false) {
                return in_use(key);
            }
            let Some(root) = root.ok().flatten().map(PathBuf::from) else {
                return format!("{key}: no tree stored; index stores it");
            };
            if !root.is_dir() {
                return format!("{key}: its tree is gone");
            }
            match phantom_fractal_git::head(&root) {
                Ok(Some(_)) => format!("{key}: HEAD reads"),
                Ok(None) => format!("{key}: not a git root"),
                Err(_) => {
                    ok = false;
                    format!("{key}: HEAD cannot be read")
                }
            }
        })
        .collect();
    Check {
        name: "git",
        ok,
        detail: if details.is_empty() {
            "no index yet".to_owned()
        } else {
            details.join("; ")
        },
    }
}
