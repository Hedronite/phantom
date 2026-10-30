//! The per-database writer lock and its read-only state probe (W9 D2).
//!
//! One `graph.db.lock` file sits beside the database: mode `0600`,
//! permanently empty, and locked with an exclusive [`std::fs::File::try_lock`]
//! for the whole of one index, watch, or registry pass. The `flock`-style
//! kernel lock frees itself when the holder dies, so a killed writer
//! leaves no stale lock behind. Readers never take it: [`lock_state`]
//! probes with a shared lock only and releases at once.
//!
//! The file carries no content, so another writer's only contention
//! evidence is the lock itself. `WriterLock` writes nothing while held.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::StoreError;

/// The lock's retry window. On contention, `acquire` retries every [
/// `RETRY_INTERVAL`] until this total has passed, then answers
/// [`StoreError::LockHeld`].
pub(crate) const RETRY_WINDOW: Duration = Duration::from_millis(250);

/// The pause between two acquire attempts.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Guard over one database's writer lock. Dropping it releases the lock;
/// the OS releases it too if the process dies.
#[derive(Debug)]
pub struct WriterLock {
    /// The held lock file handle. An open `File` keeps its input open;
    /// closing it releases the lock.
    file: std::fs::File,
}

impl WriterLock {
    /// Acquires the database's writer lock, opening or creating
    /// `graph.db.lock` beside `db_path` — mode `0600` on create — and
    /// taking an exclusive [`std::fs::File::try_lock`]. On contention the
    /// call retries every [`RETRY_INTERVAL`] for up to [`RETRY_WINDOW`],
    /// then answers `StoreError::LockHeld` (exit 4 in the CLI).
    ///
    /// The lock file is never written to and never truncated; a wider or
    /// looser existing file is narrowed to `0600` on the way in. Dropping
    /// the guard releases the lock; the OS frees it if the holder dies.
    pub fn acquire(db_path: &Path) -> Result<Self, StoreError> {
        let path = lock_path(db_path);
        let file = open_lock_file(&path)?;
        #[cfg(unix)]
        tighten(&file)?;
        let deadline = Instant::now() + RETRY_WINDOW;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(StoreError::LockHeld);
                    }
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(std::fs::TryLockError::Error(err)) => return Err(StoreError::Io(err)),
            }
        }
    }
}

impl Drop for WriterLock {
    /// Releases the lock at once; the OS would also free it at close.
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// `graph.db.lock` beside `db_path`, as `WriterLock` and [`lock_state`]
/// address it.
pub(crate) fn lock_path(db_path: &Path) -> PathBuf {
    let name = db_path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "graph.db".into());
    let mut name = name;
    name.push(".lock");
    match db_path.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Opens the lock file without creating one: `Absent`, `Held`, or
/// `Free` — never a new file.
pub(crate) fn open_existing(path: &Path) -> Result<Option<std::fs::File>, StoreError> {
    match std::fs::OpenOptions::new().read(true).open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(StoreError::Io(err)),
    }
}

/// Opens the lock file for a writer: a new one is created empty at mode
/// `0600`; an existing one opens without touching its content.
fn open_lock_file(path: &Path) -> Result<std::fs::File, StoreError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(StoreError::Io)
}

/// Narrows an existing lock file's mode to `0600` when it carries group
/// or other bits. A chmod is a metadata change, not content, so a held
/// lock is unaffected; a failure is an error.
#[cfg(unix)]
fn tighten(file: &std::fs::File) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file
        .metadata()
        .map_err(StoreError::Io)?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| {
                StoreError::Io(std::io::Error::other(
                    "lock file mode could not be narrowed",
                ))
            })?;
    }
    Ok(())
}

/// The writer lock's state, as `fract doctor` and `repos remove` read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// Another process or thread holds the exclusive lock.
    Held,
    /// The file exists and its lock is free.
    Free,
    /// No `graph.db.lock` beside the database.
    Absent,
}

/// Probes the writer lock's state without ever creating the lock file:
/// `Absent` without one, then releasing at once — so the probe never
/// holds nor waits on the lock, and leaves the database, WAL, and lock
/// file untouched.
pub fn lock_state(db_path: &Path) -> Result<LockState, StoreError> {
    let path = lock_path(db_path);
    let Some(file) = open_existing(&path)? else {
        return Ok(LockState::Absent);
    };
    match file.try_lock_shared() {
        Ok(()) => {
            file.unlock().map_err(StoreError::Io)?;
            Ok(LockState::Free)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(LockState::Held),
        Err(std::fs::TryLockError::Error(err)) => Err(StoreError::Io(err)),
    }
}
