//! The registry (SPEC-w9 G3b, D10): the indexes under the data
//! directory's `repos/`, each named by its directory, its key.
//!
//! `repos list` and the MCP `repos` tool name each index by key and by
//! its tree's directory name, never by a path. They read each index with
//! the store's guarded read-only open, which creates and changes nothing,
//! and show null for what an index in use does not let them read. `repos remove KEY` deletes
//! exactly `<data>/repos/KEY/`, under that index's writer lock. A key is
//! `^[a-z0-9-]+-[0-9a-f]{8}$`, so it can name nothing outside `repos/`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use phantom_fractal_extract::probe;
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::{SCHEMA_VERSION, Store, WriterLock, index_database_path};
use serde::Serialize;

use super::status::name;
use super::{Failure, Reply};

/// The usage message for a key that is not one.
const NOT_A_KEY: &str =
    "a key is a lowercase name, a dash and 8 hex digits, as repos list prints it";

/// `data` of `repos list` and of the MCP `repos` tool.
#[derive(Serialize)]
pub(crate) struct ReposResult {
    /// By key.
    repos: Vec<RepoEntry>,
}

/// One index. `stale` is null when its tree is gone, no tree is stored,
/// or the index cannot be read; `files` and `symbols` are null when the
/// index cannot be read.
#[derive(Serialize)]
struct RepoEntry {
    key: String,
    name: String,
    stale: Option<bool>,
    files: Option<i64>,
    symbols: Option<i64>,
}

/// `data` of `repos remove`.
#[derive(Serialize)]
pub(crate) struct Removed {
    key: String,
    removed: bool,
}

/// The data directory's `repos/`, where each index has its directory.
pub(crate) fn repos_dir() -> Option<PathBuf> {
    let database = index_database_path(Path::new("/"));
    database.parent()?.parent().map(Path::to_path_buf)
}

/// True for `^[a-z0-9-]+-[0-9a-f]{8}$`.
pub(crate) fn is_key(text: &str) -> bool {
    let Some((slug, hash)) = text.rsplit_once('-') else {
        return false;
    };
    !slug.is_empty()
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && hash.len() == 8
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The schema version in the SQLite header of `database` (its
/// `user_version`, which the store keeps equal to its stamp), read
/// without opening it, or none when it is not an SQLite file.
pub(crate) fn header_schema(database: &Path) -> Option<i64> {
    let mut head = [0u8; 64];
    fs::File::open(database).ok()?.read_exact(&mut head).ok()?;
    let version = i32::from_be_bytes([head[60], head[61], head[62], head[63]]);
    (&head[..16] == b"SQLite format 3\0").then_some(i64::from(version))
}

/// Every index under `repos/` whose directory name is a key and holds a
/// `graph.db`, by key. Nothing is written.
pub(crate) fn list() -> Result<Reply<ReposResult>, Failure> {
    let mut keys = indexes();
    keys.sort();
    let repos = keys
        .into_iter()
        .map(|key| {
            let database = repos_dir().unwrap_or_default().join(&key).join("graph.db");
            entry(key, &database)
        })
        .collect();
    Ok(Reply::plain(ReposResult { repos }))
}

/// The keys of the indexes under `repos/`, in no order.
pub(crate) fn indexes() -> Vec<String> {
    let Some(repos) = repos_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&repos) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|key| is_key(key))
        .filter(|key| {
            let dir = repos.join(key);
            fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
                && dir.join("graph.db").is_file()
        })
        .collect()
}

/// The indexes of the canonical `roots` that have one, by key, in the
/// shape of [`list`].
pub(crate) fn roots(roots: &[PathBuf]) -> Result<Reply<ReposResult>, Failure> {
    let mut repos: Vec<RepoEntry> = roots
        .iter()
        .map(|root| index_database_path(root))
        .filter(|database| database.exists())
        .filter_map(|database| {
            let key = database.parent()?.file_name()?.to_str()?.to_owned();
            Some(entry(key, &database))
        })
        .collect();
    repos.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(Reply::plain(ReposResult { repos }))
}

fn entry(key: String, database: &Path) -> RepoEntry {
    let slug = key
        .rsplit_once('-')
        .map_or(key.as_str(), |(slug, _)| slug)
        .to_owned();
    let unread = |key: String, slug: String| RepoEntry {
        name: slug,
        key,
        stale: None,
        files: None,
        symbols: None,
    };
    let Ok(store) = Store::open_read_only(database) else {
        return unread(key, slug);
    };
    let stale = store
        .root()
        .ok()
        .flatten()
        .map(PathBuf::from)
        .filter(|root| root.is_dir())
        .and_then(|root| probe(&root, &store).ok())
        .map(|freshness| freshness.stale());
    let entry = RepoEntry {
        name: name(&store, &key).unwrap_or_else(|_| slug.clone()),
        stale,
        files: store.row_count("files").ok(),
        symbols: store.row_count("symbols").ok(),
        key,
    };
    // A writer that started during the reads makes them unreliable.
    if store.read_only_still_valid().unwrap_or(false) {
        entry
    } else {
        unread(entry.key, slug)
    }
}

/// The stored tree of the index `text` names, when `text` is a key of an
/// index and not a path that exists, for the command line's `--repo`. It
/// is read with the guarded read-only open; an index in use is read with
/// the open the command itself then makes, as every read op opens it.
pub(crate) fn root_of_key(text: &Path) -> Option<PathBuf> {
    let key = text.to_str()?;
    if !is_key(key) || text.exists() {
        return None;
    }
    let database = repos_dir()?.join(key).join("graph.db");
    let guarded = Store::open_read_only(&database).ok().and_then(|store| {
        let root = store.root().ok().flatten();
        store.read_only_still_valid().ok()?.then_some(root)
    });
    let root = match guarded {
        Some(root) => root,
        None if database.is_file() && header_schema(&database) == Some(SCHEMA_VERSION) => {
            Store::open(&database).ok()?.root().ok().flatten()
        }
        None => None,
    };
    root.map(PathBuf::from)
}

/// Deletes exactly `<data>/repos/KEY/`. A key that is not one is `Usage`,
/// an unknown key `NotFound`, and a held writer lock `LockHeld`: the lock
/// is taken for the deletion, so no writer can start inside it.
pub(crate) fn remove(key: &str) -> Result<Reply<Removed>, Failure> {
    if !is_key(key) {
        return Err(Failure::new(ErrorKind::Usage, NOT_A_KEY));
    }
    let repos =
        repos_dir().ok_or_else(|| Failure::new(ErrorKind::NotFound, "no index has that key"))?;
    let is_dir = |path: &Path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    let dir = repos.join(key);
    if !is_dir(&repos) || !is_dir(&dir) {
        return Err(Failure::new(ErrorKind::NotFound, "no index has that key"));
    }
    let _lock = WriterLock::acquire(&dir.join("graph.db"))?;
    fs::remove_dir_all(&dir)
        .map_err(|_| Failure::new(ErrorKind::Usage, "the index directory could not be removed"))?;
    Ok(Reply::plain(Removed {
        key: key.to_owned(),
        removed: true,
    }))
}
