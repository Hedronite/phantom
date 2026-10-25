//! Gitignore-aware walk of one repository root (canon §6.4 step 2).

use std::fs;
use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

/// Largest file the walk keeps and [`read_source`](crate::read_source)
/// reads. Larger files are skipped, not truncated.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One file the walk kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Path relative to the walk root, `/`-separated.
    pub path: String,
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    pub size: u64,
}

/// A file the walk saw and did not keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Path relative to the walk root, `/`-separated, lossy if not UTF-8.
    pub path: String,
    pub reason: SkipReason,
}

/// Why a file was not kept. Gitignored paths, `.git`, and Cargo `target`
/// directories are pruned before they are seen and get no record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Name matches the secret denylist: `.env*`, `*.pem`, `*.key`, `id_*`.
    Secret,
    /// Larger than [`MAX_FILE_BYTES`].
    TooLarge,
    /// A symlink whose target is outside the root, or missing.
    SymlinkLeavesRoot,
    /// A symlink to a path inside the root. The target is walked itself.
    SymlinkInRoot,
    /// A symlink met when the file was opened, after the walk saw a file.
    Symlink,
    /// Not a regular file: a fifo, socket, or device.
    NotRegular,
    /// The path is not valid UTF-8.
    NonUtf8Path,
    /// Metadata could not be read.
    Unreadable,
}

/// Everything one walk found, in walk order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Walk {
    pub files: Vec<Candidate>,
    pub skipped: Vec<Skipped>,
}

/// Walks `root`.
///
/// Honours `.gitignore`, `.git/info/exclude`, and `.ignore` files in and
/// above the root, whether or not the root is a git repository. The user's
/// global gitignore is not read, so two machines walk the same tree the same
/// way. Symlinks are never followed.
pub fn walk(root: &Path) -> std::io::Result<Walk> {
    let root = root.canonicalize()?;
    let mut out = Walk::default();
    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| !is_pruned_dir(entry))
        .build();
    for entry in walker {
        match entry {
            Ok(entry) => visit(&root, &entry, &mut out),
            Err(err) => record_error(&root, &err, &mut out),
        }
    }
    Ok(out)
}

/// `.git`, and a `target` directory at the root or beside a `Cargo.toml`.
/// A `target` module elsewhere, such as `src/target/`, is walked.
fn is_pruned_dir(entry: &DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    match entry.file_name().to_str() {
        Some(".git") => true,
        Some("target") => entry
            .path()
            .parent()
            .is_some_and(|parent| entry.depth() == 1 || parent.join("Cargo.toml").is_file()),
        _ => false,
    }
}

fn visit(root: &Path, entry: &DirEntry, out: &mut Walk) {
    let Some(kind) = entry.file_type() else {
        return;
    };
    if kind.is_dir() {
        return;
    }
    let path = entry.path();
    let Some(rel) = relative(root, path) else {
        skip(out, lossy(root, path), SkipReason::NonUtf8Path);
        return;
    };
    if kind.is_symlink() {
        skip(out, rel, symlink_reason(root, path));
        return;
    }
    if is_secret(entry.file_name().to_str().unwrap_or_default()) {
        skip(out, rel, SkipReason::Secret);
        return;
    }
    match entry.metadata() {
        Ok(meta) if meta.len() > MAX_FILE_BYTES => skip(out, rel, SkipReason::TooLarge),
        Ok(meta) if meta.is_file() => out.files.push(Candidate {
            path: rel,
            abs_path: path.to_path_buf(),
            size: meta.len(),
        }),
        Ok(_) => skip(out, rel, SkipReason::NotRegular),
        Err(_) => skip(out, rel, SkipReason::Unreadable),
    }
}

fn record_error(root: &Path, err: &ignore::Error, out: &mut Walk) {
    if let Some(path) = error_path(err) {
        skip(out, lossy(root, path), SkipReason::Unreadable);
    }
}

fn error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        _ => None,
    }
}

/// True for names on the default secret denylist.
pub fn is_secret(name: &str) -> bool {
    name.starts_with(".env")
        || name.starts_with("id_")
        || name.ends_with(".pem")
        || name.ends_with(".key")
}

fn symlink_reason(root: &Path, link: &Path) -> SkipReason {
    match fs::canonicalize(link) {
        Ok(target) if target.starts_with(root) => SkipReason::SymlinkInRoot,
        _ => SkipReason::SymlinkLeavesRoot,
    }
}

fn skip(out: &mut Walk, path: String, reason: SkipReason) {
    out.skipped.push(Skipped { path, reason });
}

/// `/`-separated path relative to `root`, or `None` if it is not UTF-8.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}

fn lossy(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
