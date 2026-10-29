//! The trees a server may read (SPEC-w8 G0b).
//!
//! Each `--root` is canonicalized once, when the server starts. With no
//! `--root`, the one root is the canonical working directory. A tool's
//! `repo`, once canonicalized, must be exactly a root, and a read needs
//! that root to have an index. A missing path, a path that is not a root,
//! a root with no index, and no default all give the same failure, so a
//! client cannot tell one from another.

use std::path::{Path, PathBuf};

use phantom_fractal_proto::ErrorKind;

use crate::ops::{self, Failure};

/// The one message for every `repo` a tool may not use.
const NOT_A_ROOT: &str = "repo is not an indexed root of this server";

/// The roots, and the working directory a relative `repo` resolves
/// against.
pub(crate) struct Roots {
    /// Canonical, sorted, each once.
    roots: Vec<PathBuf>,
    /// The canonical working directory, if it has one.
    cwd: Option<PathBuf>,
}

impl Roots {
    /// The roots `given` names, or the working directory when none is
    /// given. A root that is not a directory is an error that gives its
    /// position, never its path.
    pub(crate) fn new(given: &[PathBuf]) -> Result<Self, String> {
        let cwd = std::env::current_dir()
            .and_then(|dir| dir.canonicalize())
            .ok();
        let mut roots = if given.is_empty() {
            cwd.iter().cloned().collect()
        } else {
            given
                .iter()
                .enumerate()
                .map(|(position, root)| {
                    root.canonicalize()
                        .ok()
                        .filter(|root| root.is_dir())
                        .ok_or_else(|| {
                            format!(
                                "usage: --root {} of {} is not a directory",
                                position + 1,
                                given.len()
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        roots.sort();
        roots.dedup();
        Ok(Self { roots, cwd })
    }

    /// The root `repo` names, or the default root when there is no `repo`:
    /// the only root, or else the working directory when it is a root.
    pub(crate) fn root(&self, repo: Option<&str>) -> Result<PathBuf, Failure> {
        let path = match repo {
            Some(repo) => Path::new(repo).canonicalize().ok(),
            None => match self.roots.as_slice() {
                [only] => Some(only.clone()),
                _ => self.cwd.clone(),
            },
        };
        path.filter(|path| self.roots.contains(path))
            .ok_or_else(not_a_root)
    }

    /// [`Roots::root`], when that root has an index.
    pub(crate) fn indexed(&self, repo: Option<&str>) -> Result<PathBuf, Failure> {
        let root = self.root(repo)?;
        if ops::has_index(&root) {
            Ok(root)
        } else {
            Err(not_a_root())
        }
    }
}

fn not_a_root() -> Failure {
    Failure::new(ErrorKind::NoIndex, NOT_A_ROOT)
}
