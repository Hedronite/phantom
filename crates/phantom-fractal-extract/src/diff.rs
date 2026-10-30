//! The symbols a work-tree change touches (SPEC-w9 G2a, D8).
//!
//! [`diff`] walks the root with the indexer's own walk and read, asks
//! [`phantom_fractal_git::changes`] which kept files differ from the base
//! commit, and extracts each changed `.rs` file's base bytes and work-tree
//! bytes in memory with the indexer's extractor. Nothing is written: not
//! the store, not the tree, not `.git`.
//!
//! A symbol is named by the stable key the store gives it. It is added
//! when its key is only in the work tree, removed when only in the base,
//! and modified when the bytes of its lines differ. Moving a symbol
//! without editing it is no change; renaming it is a removal and an
//! addition.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use phantom_fractal_git::{Base, ChangesError, FileChange, changes};
use phantom_fractal_lang::{Extract, ExtractError, Language};
use phantom_fractal_lang_rust::RustLanguage;

use crate::is_rust;
use crate::read::{Source, read_source};
use crate::walk::{MAX_FILE_BYTES, is_secret, walk};

/// One changed symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedSymbol {
    /// The symbol's file, repo-relative.
    pub path: String,
    /// The stable key the store gives the symbol.
    pub key: String,
    /// The symbol's own name; a method's, without its owner.
    pub name: String,
    pub change: FileChange,
}

/// A changed file listed by itself: one that is not `.rs`, or a `.rs`
/// file whose change touches no symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOnly {
    pub path: String,
    pub change: FileChange,
}

/// What [`diff`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// The base commit, 40 hex.
    pub base_oid: String,
    /// Sorted by path, then key.
    pub changed: Vec<ChangedSymbol>,
    /// Sorted by path.
    pub files_only: Vec<FileOnly>,
    /// A `Cargo.toml` is among the changed files.
    pub manifest_changed: bool,
}

/// Why [`diff`] stopped.
#[derive(Debug)]
#[non_exhaustive]
pub enum DiffError {
    /// The root could not be walked, or a kept file could not be read.
    Io(std::io::Error),
    /// The base names no commit, or more than one, or the repository
    /// could not be read.
    Changes(ChangesError),
    /// The extractor refused a file.
    Extract(ExtractError),
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "walk failed: {err}"),
            Self::Changes(err) => write!(f, "{err}"),
            Self::Extract(err) => write!(f, "extract: {err}"),
        }
    }
}

impl std::error::Error for DiffError {}

impl From<std::io::Error> for DiffError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<ChangesError> for DiffError {
    fn from(err: ChangesError) -> Self {
        Self::Changes(err)
    }
}

impl From<ExtractError> for DiffError {
    fn from(err: ExtractError) -> Self {
        Self::Extract(err)
    }
}

/// The changed symbols and files of the work tree at `root` against
/// `base`. `Ok(None)` when `root` is not the top of a git work tree with a
/// born HEAD, exactly as for [`phantom_fractal_git::head`].
///
/// The work tree's side is what an index of it keeps: every file the walk
/// keeps and the bounded read reads, and of the `.rs` files only those
/// that are UTF-8. A base file counts when an index would have kept it:
/// at most [`MAX_FILE_BYTES`], not secret-named, not under `.git` or a
/// pruned `target`, and kept in the work tree, gone from it, or seen by
/// the walk and not kept (now too large, a link, not UTF-8, and so on),
/// which removes it. A base path the walk never sees although it is
/// there, an ignored one, is no change: it is not indexed on either side.
pub fn diff(root: &Path, base: &Base, language: &RustLanguage) -> Result<Option<Diff>, DiffError> {
    let root = root.canonicalize()?;
    let walked = walk(&root)?;
    let seen: HashSet<String> = walked
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(walked.skipped.iter().map(|skip| skip.path.clone()))
        .collect();
    let mut worktree: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for file in walked.files {
        let Source::Bytes { bytes, .. } = read_source(&file.abs_path)? else {
            continue;
        };
        if is_rust(&file.path) && std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        worktree.insert(file.path, bytes);
    }
    let keep_base = |path: &str, size: u64| {
        size <= MAX_FILE_BYTES
            && !path.rsplit('/').next().is_some_and(is_secret)
            && !is_pruned(&root, path)
            && (worktree.contains_key(path)
                || seen.contains(path)
                || fs::symlink_metadata(root.join(path)).is_err())
    };
    let Some(found) = changes(&root, base, &worktree, &keep_base)? else {
        return Ok(None);
    };
    let mut out = Diff {
        base_oid: found.base_oid,
        changed: Vec::new(),
        files_only: Vec::new(),
        manifest_changed: false,
    };
    for file in found.files {
        if file.path == "Cargo.toml" || file.path.ends_with("/Cargo.toml") {
            out.manifest_changed = true;
        }
        let before = out.changed.len();
        if is_rust(&file.path) {
            let old = spans(&file.path, file.base.as_deref(), language)?;
            let new = match file.change {
                FileChange::Removed => BTreeMap::new(),
                _ => spans(
                    &file.path,
                    worktree.get(&file.path).map(Vec::as_slice),
                    language,
                )?,
            };
            out.changed.extend(compare(&file.path, &old, &new));
        }
        if out.changed.len() == before {
            out.files_only.push(FileOnly {
                path: file.path,
                change: file.change,
            });
        }
    }
    out.changed
        .sort_by(|a, b| (&a.path, &a.key).cmp(&(&b.path, &b.key)));
    Ok(Some(out))
}

/// A symbol's name and the bytes of its lines, by stable key.
type Spans = BTreeMap<String, (String, Vec<u8>)>;

/// The symbols of `bytes` as the file at `path`, or none for no bytes or
/// bytes that are not UTF-8.
fn spans(path: &str, bytes: Option<&[u8]>, language: &RustLanguage) -> Result<Spans, DiffError> {
    let Some(text) = bytes.and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
        return Ok(Spans::new());
    };
    let extract = language.extract(text)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    Ok(symbol_keys(path, &extract)
        .into_iter()
        .map(|(key, name, start, end)| (key, (name, span(&lines, start, end))))
        .collect())
}

/// The changes between two files' symbols, in key order.
fn compare(path: &str, old: &Spans, new: &Spans) -> Vec<ChangedSymbol> {
    let mut out = Vec::new();
    for (key, (name, bytes)) in old {
        let change = match new.get(key) {
            None => FileChange::Removed,
            Some((_, now)) if now != bytes => FileChange::Modified,
            Some(_) => continue,
        };
        out.push(changed(path, key, name, change));
    }
    for (key, (name, _)) in new {
        if !old.contains_key(key) {
            out.push(changed(path, key, name, FileChange::Added));
        }
    }
    out
}

fn changed(path: &str, key: &str, name: &str, change: FileChange) -> ChangedSymbol {
    ChangedSymbol {
        path: path.to_owned(),
        key: key.to_owned(),
        name: name.to_owned(),
        change,
    }
}

/// Every symbol of `extract` as `(stable key, name, start line, end
/// line)`, in the store's insert order: items, then methods. The keys are
/// the store's (`put_source`): `<path>#<name>#<kind>#<n>` for an item and
/// `<path>#<owner>::<name>#method#<n>` for a method, where `n` counts the
/// earlier items of that name and kind, or the earlier methods of that
/// name.
pub fn symbol_keys(path: &str, extract: &Extract) -> Vec<(String, String, u32, u32)> {
    let items = extract.items.iter().enumerate().map(|(position, item)| {
        let ordinal = extract.items[..position]
            .iter()
            .filter(|prior| prior.name == item.name && prior.kind == item.kind)
            .count();
        (
            format!("{path}#{}#{}#{ordinal}", item.name, item.kind.as_str()),
            item.name.clone(),
            item.start_line,
            item.end_line,
        )
    });
    let methods = extract
        .methods
        .iter()
        .enumerate()
        .map(|(position, method)| {
            let ordinal = extract.methods[..position]
                .iter()
                .filter(|prior| prior.name == method.name)
                .count();
            (
                format!("{path}#{}::{}#method#{ordinal}", method.owner, method.name),
                method.name.clone(),
                method.start_line,
                method.end_line,
            )
        });
    items.chain(methods).collect()
}

/// The bytes of lines `start..=end`, counted from 1.
fn span(lines: &[&str], start: u32, end: u32) -> Vec<u8> {
    let start = (start as usize).saturating_sub(1);
    let end = (end as usize).min(lines.len());
    lines
        .get(start..end)
        .map(|lines| lines.concat().into_bytes())
        .unwrap_or_default()
}

/// True for a path the walk prunes: under `.git`, or under a `target`
/// directory at the root or beside a `Cargo.toml` in the work tree.
fn is_pruned(root: &Path, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let dirs = &parts[..parts.len().saturating_sub(1)];
    dirs.iter().enumerate().any(|(depth, part)| match *part {
        ".git" => true,
        "target" => {
            depth == 0
                || root
                    .join(dirs[..depth].join("/"))
                    .join("Cargo.toml")
                    .is_file()
        }
        _ => false,
    })
}
