//! Read-only git co-change mining (SPEC-w8 G1c, canon §7.2).
//!
//! [`mine`] opens a root's repository isolated (no system, global or
//! environment config, no includes, no `git` binary), walks the
//! first-parent chain of HEAD inside a [`Window`], diffs each commit's
//! tree against its first parent's, follows renames to current paths,
//! and scores every pair of indexed files that changed together. The
//! result is the store's [`Scan`], ready for `write_cochange`.
//!
//! It only reads objects and refs. It never writes to the tree or `.git`,
//! never searches upward from the root, spawns no process, and reads no
//! author, email or message text: of each commit it takes the committer
//! time, the tree and the parent ids.
//!
//! gix calls are limited to the G1a allowlist ([`history`] holds all of
//! them). Rename tracking is Phantom's own ([`rename`]), because gix's
//! needs features the D2 amendment bans.
#![forbid(unsafe_code)]

mod history;
mod rename;
mod score;

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::Path;

pub use phantom_fractal_store::CochangeScan as Scan;
pub use rename::{MIN_SIMILARITY_PERCENT, similarity};
pub use score::{BULK_FILES, HALF_LIFE_DAYS, MIN_TOGETHER, TOP_PARTNERS, decay};

/// How much history [`mine`] reads (D4): the first-parent chain of HEAD,
/// newest first, stopping at `max_commits` commits or at the first commit
/// older than HEAD's committer time minus `days`, whichever comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// 1 to [`Window::MAX_DAYS`]; the caller checks the range.
    pub days: u32,
    /// 1 to [`Window::MAX_COMMITS`]; the caller checks the range.
    pub max_commits: u32,
}

impl Window {
    pub const DEFAULT_DAYS: u32 = 365;
    pub const DEFAULT_COMMITS: u32 = 5_000;
    pub const MAX_DAYS: u32 = 3_650;
    pub const MAX_COMMITS: u32 = 50_000;

    /// True when both fields are in their ranges.
    pub fn in_range(&self) -> bool {
        (1..=Self::MAX_DAYS).contains(&self.days)
            && (1..=Self::MAX_COMMITS).contains(&self.max_commits)
    }

    /// The canonical `window_json`: `{"days":D,"max_commits":C}`.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"days\":{},\"max_commits\":{}}}",
            self.days, self.max_commits
        )
    }
}

impl Default for Window {
    /// 365 days, 5,000 commits.
    fn default() -> Self {
        Self {
            days: Self::DEFAULT_DAYS,
            max_commits: Self::DEFAULT_COMMITS,
        }
    }
}

/// A repository that opened as a root but could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(String);

impl GitError {
    pub(crate) fn new(err: impl fmt::Display) -> Self {
        Self(err.to_string())
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git history could not be read: {}", self.0)
    }
}

impl Error for GitError {}

/// The co-change scan of `root`, counting only paths in `indexed_paths`
/// (root-relative, `/`-separated, as the store keeps them).
///
/// `Ok(None)` when `root` is not the top of a git work tree: it has no
/// `.git`, the repository does not open, its work tree is elsewhere (so
/// `root` is a subdirectory or the repository is bare), or HEAD is
/// unborn. The same repository, HEAD and window always give an equal scan.
pub fn mine(
    root: &Path,
    window: &Window,
    indexed_paths: &BTreeSet<String>,
) -> Result<Option<Scan>, GitError> {
    let Some(history) = history::read(root, window)? else {
        return Ok(None);
    };
    Ok(Some(score::score(&history, window, indexed_paths)))
}
