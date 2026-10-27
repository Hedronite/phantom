//! The `0700` directory chain on the way to `graph.db`, and the open-time
//! guards over it.
//!
//! W2 created the chain and narrowed `repos/` and the repo directory.
//! W3 adds two refusals (SecOps review #15): a symlinked or non-directory member
//! makes open fail before any `chmod`, and every member must be owned by
//! the process's effective uid.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use crate::StoreError;

/// Mode for every directory the store creates and for `repos/` and the
/// repo directory after a tighten.
pub(crate) const DIR_MODE: u32 = 0o700;

/// The store's directory chain when `graph.db` sits in the recognized
/// `repos/<name>-<hash>/` layout.
#[cfg(unix)]
pub(crate) struct Chain {
    pub(crate) data_root: PathBuf,
    pub(crate) repos: PathBuf,
    pub(crate) repo_dir: PathBuf,
}

#[cfg(unix)]
impl Chain {
    /// The chain of `path`, or `None` when no ancestor is named `repos`,
    /// so a plain database keeps the W2 behavior untouched.
    pub(crate) fn new(path: &Path) -> Option<Self> {
        let repo_dir = path.parent()?;
        let repos = repo_dir.parent()?;
        if repos.file_name() != Some(std::ffi::OsStr::new("repos")) {
            return None;
        }
        Some(Self {
            data_root: repos.parent()?.to_path_buf(),
            repos: repos.to_path_buf(),
            repo_dir: repo_dir.to_path_buf(),
        })
    }
}

/// Creates every directory the store is missing on the way to the
/// database with a recursive [`std::fs::DirBuilder`] at mode `0o700`.
///
/// [`std::fs::DirBuilder`] only assigns the mode to directories it
/// creates, so a data root that already existed — say a
/// `PHANTOM_FRACTAL_DATA` the user pointed at — keeps its mode here.
#[cfg(unix)]
pub(crate) fn create_private_directories(path: &Path) -> Result<(), StoreError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut dirs = std::fs::DirBuilder::new();
        dirs.recursive(true);
        dirs.mode(DIR_MODE);
        dirs.create(parent).map_err(StoreError::Io)?;
    }
    Ok(())
}

/// Refuses, for each chain member that exists, a symlink or a
/// non-directory. `symlink_metadata` never follows a link, so a
/// `repos/` pointing at a directory elsewhere is caught here and nothing
/// above or below it is touched (SecOps review #15 N1).
#[cfg(unix)]
pub(crate) fn verify_shape(chain: &Chain) -> Result<(), StoreError> {
    for dir in [&chain.repo_dir, &chain.repos] {
        if let Ok(meta) = std::fs::symlink_metadata(dir)
            && !meta.is_dir()
        {
            return Err(StoreError::DataDirectory(
                map_path(dir),
                "is a symlink or not a directory".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Narrows `repos/` and the repo directory to [`DIR_MODE`] when either
/// still has group or other bits. Each member is read with
/// The same four steps for `repos/` and the repo directory. A symlink or
/// non-directory still fails as W3 does, and the chmod lands only after
/// the same inode verified by `symlink_metadata` shows up under the open
/// handle (fstat: same `dev` and `ino`, a directory, owned by the
/// effective uid) — so a swap between check and open is refused and
/// nothing is chmodded through a link or to a different directory
/// (SecOps review #15 N3). Anything above `repos/` — the data root in
/// particular — is never changed.
#[cfg(unix)]
pub(crate) fn tighten_directories(chain: &Chain, effective_uid: u32) -> Result<(), StoreError> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    for dir in [&chain.repo_dir, &chain.repos] {
        let checked = std::fs::symlink_metadata(dir).map_err(StoreError::Io)?;
        if !checked.is_dir() {
            return Err(StoreError::DataDirectory(
                map_path(dir),
                "is a symlink or not a directory".to_owned(),
            ));
        }
        #[cfg(test)]
        race_window(dir);
        let file = std::fs::File::open(dir).map_err(|err| {
            StoreError::DataDirectory(map_path(dir), format!("not openable ({err})"))
        })?;
        let opened = file.metadata().map_err(StoreError::Io)?;
        let same = opened.dev() == checked.dev() && opened.ino() == checked.ino();
        if !same || !opened.is_dir() || !owned_by(&opened, effective_uid) {
            return Err(StoreError::DataDirectory(
                map_path(dir),
                "changed between the check and the open".to_owned(),
            ));
        }
        let mode = opened.permissions().mode();
        if mode & 0o077 == 0 {
            continue;
        }
        file.set_permissions(std::fs::Permissions::from_mode(DIR_MODE))
            .map_err(StoreError::Io)?;
    }
    Ok(())
}

/// The one race window between the metadata check and the open. Built
/// only into test builds; a test injects a swap with [`swap_under_open`]
/// and clears it again after (SecOps review #15 N3).
#[cfg(test)]
fn race_window(dir: &Path) {
    SWAP.with_borrow(|slot| {
        if let Some(action) = slot {
            action(dir);
        }
    });
}

#[cfg(test)]
type Swap = Box<dyn Fn(&Path)>;

#[cfg(test)]
thread_local! {
    static SWAP: std::cell::RefCell<Option<Swap>> = const { std::cell::RefCell::new(None) };
}

/// Injects (or clears, with `None`) a test-only swap between the
/// metadata check and the open; the swap runs on the caller's thread.
#[cfg(test)]
pub(crate) fn swap_under_open(hook: Option<Swap>) {
    SWAP.with_borrow_mut(|slot| *slot = hook);
}

/// Owner comparison for the chain (SecOps review #15 N2). Pure on the metadata:
/// a member qualifies only when its uid equals `effective_uid`.
#[cfg(unix)]
pub(crate) fn owned_by(meta: &std::fs::Metadata, effective_uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.uid() == effective_uid
}

/// Reads the process's effective uid without any `unsafe` and without
/// reading the real uid: a file the store itself creates along
/// `create_new` is owned by the effective uid, so its owner is the
/// answer. The probe is created inside the store's own repo directory
/// and removed again; a failure to create it means the chain sits in
/// something the effective uid does not own, which is a refusal.
#[cfg(unix)]
pub(crate) fn effective_uid(dir: &Path, probe: &Path) -> Result<u32, StoreError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(probe)
        .map_err(|err| {
            StoreError::NotOwnedByUs(
                map_path(dir),
                format!("no owner probe writable for the effective uid ({err})"),
            )
        })?;
    let uid = std::fs::metadata(probe)
        .map_err(StoreError::Io)
        .map(|meta| {
            use std::os::unix::fs::MetadataExt;
            meta.uid()
        });
    std::fs::remove_file(probe).map_err(StoreError::Io)?;
    uid
}

/// Refuses any chain member that exists but is owned by a uid other than
/// `effective_uid` (SecOps review #15 N2). A member that does not yet exist is
/// created by this process and owned.
#[cfg(unix)]
pub(crate) fn verify_owned(chain: &Chain, effective_uid: u32) -> Result<(), StoreError> {
    for dir in [&chain.data_root, &chain.repos, &chain.repo_dir] {
        if let Ok(meta) = std::fs::symlink_metadata(dir)
            && !owned_by(&meta, effective_uid)
        {
            return Err(StoreError::NotOwnedByUs(
                map_path(dir),
                "owned by another uid".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn map_path(path: &Path) -> String {
    // An error message never carries an absolute path.
    match path.file_name().and_then(std::ffi::OsStr::to_str) {
        Some(name) => name.to_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}
