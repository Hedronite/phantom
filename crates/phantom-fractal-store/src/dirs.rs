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

/// The checked-dir open flags, per target. On these three targets the
/// kernel itself refuses symlinks and non-directories — no follow ever
/// happens at the open (SecOps review #21 N1). The values differ by
/// arch, so they are fixed per cfg with the system header named, and a
/// compile-time assert pins each pair: distinct, and equal to the
/// header value. Linux x86_64 takes `O_DIRECTORY` `0o200000` and
/// `O_NOFOLLOW` `0o400000` from `bits/fcntl-linux.h` (exposed by
/// `<fcntl.h>`). Linux aarch64 takes `O_DIRECTORY` `0o40000` and
/// `O_NOFOLLOW` `0o100000` from the arm64 wiring of
/// `<asm-generic/fcntl.h>` — the same numbers the vendored libc crate
/// carries in its gnu b64 aarch64 module. macOS takes
/// `O_DIRECTORY` `0x00100000` and `O_NOFOLLOW` `0x00000100` from
/// `<sys/fcntl.h>`. The kernel-level check is redundant with the
/// `dev`/`ino` compare below, which stays on every target, so other
/// unix targets keep the plain open. No `libc` call: the values are
/// constants, not a link.
#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
))]
mod checked_flags {
    /// `O_DIRECTORY` for Linux x86_64 (`bits/fcntl-linux.h`: `0o200000`).
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub(crate) const O_DIRECTORY: i32 = 0o200000;
    /// `O_NOFOLLOW` for Linux x86_64 (`bits/fcntl-linux.h`: `0o400000`).
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub(crate) const O_NOFOLLOW: i32 = 0o400000;
    /// `O_DIRECTORY` for Linux aarch64 (arm64
    /// `<asm-generic/fcntl.h>`: `0o40000`).
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    pub(crate) const O_DIRECTORY: i32 = 0o40000;
    /// `O_NOFOLLOW` for Linux aarch64 (`0o100000`).
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    pub(crate) const O_NOFOLLOW: i32 = 0o100000;
    /// `O_DIRECTORY` for macOS (`<sys/fcntl.h>`: `0x00100000`).
    #[cfg(target_os = "macos")]
    pub(crate) const O_DIRECTORY: i32 = 0x00100000;
    /// `O_NOFOLLOW` for macOS (`0x00000100`).
    #[cfg(target_os = "macos")]
    pub(crate) const O_NOFOLLOW: i32 = 0x00000100;

    const _: () = {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert!(
            O_DIRECTORY == 0o200000 as i32
                && O_NOFOLLOW == 0o400000 as i32
                && O_DIRECTORY != O_NOFOLLOW
        );
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        assert!(
            O_DIRECTORY == 0o40000 as i32
                && O_NOFOLLOW == 0o100000 as i32
                && O_DIRECTORY != O_NOFOLLOW
        );
        #[cfg(target_os = "macos")]
        assert!(O_DIRECTORY == 0x00100000 && O_NOFOLLOW == 0x00000100 && O_DIRECTORY != O_NOFOLLOW);
    };
}

/// Opens the checked directory. On the flagged targets the open itself
/// refuses symlinks (`O_NOFOLLOW`) and non-directories (`O_DIRECTORY`);
/// elsewhere the plain open plus the fstat compare below keep the same
/// refusals. The error carries the mapped path and the
/// `io::ErrorKind` text only, never the raw OS message (SecOps review
/// #21 N2).
#[cfg(unix)]
fn open_checked(dir: &Path) -> Result<std::fs::File, StoreError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(checked_flags::O_DIRECTORY | checked_flags::O_NOFOLLOW);
    }
    options.open(dir).map_err(|err| {
        StoreError::DataDirectory(map_path(dir), format!("not openable ({})", err.kind()))
    })
}

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
        let file = open_checked(dir)?;
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
