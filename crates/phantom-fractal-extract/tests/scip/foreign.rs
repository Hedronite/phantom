//! The foreign-root scope and the W13 miss context (W13 G0a, D4).
//!
//! [`Members`] reads the member list out of the `cargo metadata
//! --no-deps --format-version 1` file the runner passes through
//! `PHANTOM_SCORE_MEMBERS`: every package's `manifest_path`, made
//! relative to the file's `workspace_root`. A document outside every
//! member directory leaves both the truth set and the edge set. The
//! reader is a hand scan of the two fields — the test spawns no cargo
//! and pulls in no JSON crate.
//!
//! [`MissContext`] carries the store facts the three new miss buckets
//! read: the files more than one crate target claims (`shared module
//! file`), the module rows each glob `use` reaches (`glob import in
//! scope`), and the caller-scope `use` bindings whose import target
//! lives in a module other than the one the binding's own path names
//! (`re-exported path`). The rules are the store-side mirror of what
//! the resolver sees, and every choice is documented where it is
//! made.

use std::collections::{BTreeMap, BTreeSet};

use phantom_fractal_store::ModuleRow;

/// The workspace members, as member directories relative to the
/// workspace root.
pub struct Members {
    dirs: Vec<String>,
}

impl Members {
    /// Parses the runner's metadata file. Every `manifest_path` the
    /// file carries is a member (`--no-deps` lists workspace members
    /// only); its directory is read relative to `workspace_root`.
    pub fn from_metadata(bytes: &[u8]) -> Result<Members, String> {
        let root = string_values(bytes, b"\"workspace_root\":\"")
            .first()
            .cloned()
            .ok_or("the metadata file carries no workspace_root")?;
        let mut dirs = Vec::new();
        for value in string_values(bytes, b"\"manifest_path\":\"") {
            let dir = match value.rsplit_once('/') {
                Some((head, _)) => head.to_owned(),
                None => String::new(),
            };
            let rel = dir
                .strip_prefix(root.as_str())
                .map(|left| left.trim_start_matches('/').to_owned())
                .unwrap_or_else(|| dir.clone());
            dirs.push(rel);
        }
        dirs.sort();
        Ok(Members { dirs })
    }

    /// True when the document sits inside some member's directory. A
    /// member at the workspace root reads as an empty directory and
    /// claims everything.
    pub fn in_scope(&self, document: &str) -> bool {
        self.dirs.iter().any(|dir| {
            dir.is_empty() || document == dir.as_str() || document.starts_with(&format!("{dir}/"))
        })
    }
}

/// Every string value that starts right after `key`, honoring the two
/// escapes JSON writes for paths (`\\` and `\"`).
fn string_values(bytes: &[u8], key: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while let Some(at) = rest.windows(key.len()).position(|window| window == key) {
        let mut pos = at + key.len();
        let mut value = Vec::new();
        while pos < rest.len() {
            match rest[pos] {
                b'\\'
                    if pos + 1 < rest.len()
                        && (rest[pos + 1] == b'\\' || rest[pos + 1] == b'"') =>
                {
                    value.push(rest[pos + 1]);
                    pos += 2;
                }
                b'"' => break,
                byte => {
                    value.push(byte);
                    pos += 1;
                }
            }
        }
        out.push(String::from_utf8_lossy(&value).into_owned());
        rest = &rest[pos..];
    }
    out
}

/// One caller-scope `use` binding whose import edge carries a target:
/// the name it binds, the module path its written path names before
/// the last segment, and the home module path of that target.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub bound: String,
    pub prefix: String,
    pub home: String,
}

/// The store facts the three new miss buckets read, resolved once per
/// score run.
#[derive(Debug, Clone, Default)]
pub struct MissContext {
    /// Files that more than one crate target claims, by file id.
    pub shared: BTreeSet<i64>,
    /// The module rows each glob `use` reaches, by the (file id,
    /// scope) the use sits in.
    pub glob: BTreeMap<(i64, String), BTreeSet<i64>>,
    /// The module row of each symbol that carries one.
    pub symbol_module: BTreeMap<i64, i64>,
    /// The non-glob `use` bindings with a resolved import target, by
    /// the (file id, scope) they sit in.
    pub uses: BTreeMap<(i64, String), Vec<Binding>>,
}

impl MissContext {
    /// Builds the context from the store's module, use and import
    /// reads.
    pub fn from_store(store: &phantom_fractal_store::Store) -> Result<MissContext, String> {
        let module_rows = store.module_rows().map_err(|err| format!("store: {err}"))?;
        let targets = store
            .file_target_counts()
            .map_err(|err| format!("store: {err}"))?;
        let shared: BTreeSet<i64> = targets
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(file_id, _)| file_id)
            .collect();

        // The crate root row of each target: the row with no parent.
        // The row an inline scope answers: the row with that file and
        // scope; a file whose root scope carries no row falls back to
        // its file-root row, or the use is skipped.
        let mut roots: BTreeMap<i64, i64> = BTreeMap::new();
        let mut inline: BTreeMap<(i64, String), i64> = BTreeMap::new();
        for (id, row) in &module_rows {
            if let Some(file_id) = row.file_id {
                inline.entry((file_id, row.scope.clone())).or_insert(*id);
            }
            if row.parent_id.is_none() {
                roots.entry(row.target_id).or_insert(*id);
            }
        }

        let use_rows = store.use_rows().map_err(|err| format!("store: {err}"))?;
        let imports = store
            .import_targets()
            .map_err(|err| format!("store: {err}"))?;
        let symbol_modules = store
            .symbol_modules()
            .map_err(|err| format!("store: {err}"))?;

        let mut context = MissContext {
            shared,
            ..MissContext::default()
        };

        for use_row in &use_rows {
            let start = match inline.get(&(use_row.file_id, use_row.scope.clone())) {
                Some(id) => *id,
                None => {
                    let found = module_rows
                        .iter()
                        .find(|(_, row)| {
                            row.file_id == Some(use_row.file_id) && row.scope.is_empty()
                        })
                        .map(|(id, _)| *id);
                    let Some(found) = found else { continue };
                    found
                }
            };
            if use_row.glob {
                if let Some(reached) = resolve_glob(&use_row.text, start, &module_rows, &roots) {
                    context
                        .glob
                        .entry((use_row.file_id, use_row.scope.clone()))
                        .or_default()
                        .insert(reached);
                }
                continue;
            }
            let Some((kind, id)) = imports.get(&use_row.id) else {
                continue;
            };
            let Some(home) = home_path(kind, *id, &symbol_modules, &module_rows) else {
                continue;
            };
            let bound = use_row
                .alias
                .clone()
                .unwrap_or_else(|| last_segment(&use_row.text).to_owned());
            let prefix = match use_row.text.rsplit_once("::") {
                Some((head, _)) => head.to_owned(),
                None => String::new(),
            };
            context
                .uses
                .entry((use_row.file_id, use_row.scope.clone()))
                .or_default()
                .push(Binding {
                    bound,
                    prefix,
                    home,
                });
        }
        context.symbol_module = symbol_modules;
        Ok(context)
    }

    /// True when the callee symbol is defined in a module row that a
    /// glob use in the caller's scope reaches (W13 D4, `glob import in
    /// scope`).
    pub fn glob_reaches(&self, file_id: i64, scope: &str, callee_id: i64) -> bool {
        let Some(home) = self.symbol_module.get(&callee_id) else {
            return false;
        };
        self.glob
            .get(&(file_id, scope.to_owned()))
            .is_some_and(|reached| reached.contains(home))
    }

    /// True when the call text's first segment names its item through
    /// a `pub use` in another module: a caller-scope binding whose
    /// target's home module path differs from the module path the
    /// binding's own text names before the last segment (W13 D4,
    /// `re-exported path`).
    pub fn reexported(&self, file_id: i64, scope: &str, text: &str) -> bool {
        let Some((first, _)) = text.split_once("::") else {
            return false;
        };
        self.uses
            .get(&(file_id, scope.to_owned()))
            .is_some_and(|bindings| {
                bindings
                    .iter()
                    .any(|binding| binding.bound == first && binding.home != binding.prefix)
            })
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// Walks a glob's written path to the id of the module row it
/// reaches. `super` moves to the row's parent, `self` stays, `crate`
/// and a bare first segment start at the crate root row (the bare
/// form is crate-root-relative, the 2018 reading), `std`, `core` and
/// `alloc` reach no workspace module, and each following segment
/// names a child row by its parent and its path's last segment.
fn resolve_glob(
    path: &str,
    start: i64,
    module_rows: &BTreeMap<i64, ModuleRow>,
    roots: &BTreeMap<i64, i64>,
) -> Option<i64> {
    let mut segments: Vec<&str> = path.split("::").collect();
    let start_row = module_rows.get(&start)?;
    let mut row = start;
    match segments.first().copied() {
        Some("super") => {
            row = start_row.parent_id?;
            segments.remove(0);
        }
        Some("self") => {
            segments.remove(0);
        }
        Some("crate") => {
            row = *roots.get(&start_row.target_id)?;
            segments.remove(0);
        }
        Some("std" | "core" | "alloc") => return None,
        _ => {
            row = *roots.get(&start_row.target_id)?;
        }
    }
    for segment in segments {
        row = module_rows
            .iter()
            .find(|(_, candidate)| {
                candidate.parent_id == Some(row) && last_segment(&candidate.path) == segment
            })
            .map(|(id, _)| *id)?;
    }
    Some(row)
}

/// The home module path of an import target: the symbol's module
/// row's path, or the module row's own path.
fn home_path(
    kind: &str,
    id: i64,
    symbol_modules: &BTreeMap<i64, i64>,
    module_rows: &BTreeMap<i64, ModuleRow>,
) -> Option<String> {
    let module = match kind {
        "symbol" => symbol_modules.get(&id).copied()?,
        _ => id,
    };
    module_rows.get(&module).map(|row| row.path.clone())
}
