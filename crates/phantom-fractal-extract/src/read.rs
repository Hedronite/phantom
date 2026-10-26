//! Bounded, no-follow read of one walked file.
//!
//! The walk classifies each entry from its metadata, and the file can change
//! before it is read. The read re-checks on the open descriptor, so a path
//! swapped for a symlink, a fifo, a hard link, or a longer file is still not
//! read as source.

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::walk::{MAX_FILE_BYTES, SkipReason};

/// What [`read_source`] found at one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The whole file, at most [`MAX_FILE_BYTES`] long.
    Bytes {
        bytes: Vec<u8>,
        /// Modification time in nanoseconds since the epoch, or 0 if unknown.
        mtime_ns: i64,
    },
    /// The file was not read as source.
    Skipped(SkipReason),
}

/// Reads `path` as source.
///
/// The open uses `O_NOFOLLOW`, so a symlink is refused rather than followed.
/// It also uses `O_NONBLOCK`, so opening a fifo does not wait for a writer.
/// The descriptor must then be a regular file with one link. At most `MAX_FILE_BYTES + 1`
/// bytes are read; if that many come back, the file is skipped whole.
pub fn read_source(path: &Path) -> io::Result<Source> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.raw_os_error() == Some(libc::ELOOP) => {
            return Ok(Source::Skipped(SkipReason::Symlink));
        }
        Err(err) => return Err(err),
    };
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Ok(Source::Skipped(SkipReason::NotRegular));
    }
    if meta.nlink() > 1 {
        return Ok(Source::Skipped(SkipReason::HardLink));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if !u64::try_from(bytes.len()).is_ok_and(|len| len <= MAX_FILE_BYTES) {
        return Ok(Source::Skipped(SkipReason::TooLarge));
    }
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|dur| i64::try_from(dur.as_nanos()).ok())
        .unwrap_or(0);
    Ok(Source::Bytes { bytes, mtime_ns })
}
