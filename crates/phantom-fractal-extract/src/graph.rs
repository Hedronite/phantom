//! Packages, targets, and the module tree (SPEC-w3 G2a).
//!
//! A manifest is scanned line by line: no TOML crate. Modules follow the
//! Rust 2018 file rules from each target root. Nothing is compiled or run.

use std::collections::{HashMap, HashSet, VecDeque};

use phantom_fractal_lang::{Id, ModDecl, Module, Target, TargetKind};
use phantom_fractal_store::Package;

/// One kept `.rs` file and the `mod` declarations it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Repo-relative, `/`-separated.
    pub path: String,
    pub mod_decls: Vec<ModDecl>,
}

/// One kept `Cargo.toml` and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Repo-relative, `/`-separated.
    pub path: String,
    pub text: String,
}

/// The crate graph of one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub packages: Vec<Package>,
    pub targets: Vec<Target>,
    pub modules: Vec<Module>,
    /// Every module whose declaration carries `#[cfg(test)]`, and every
    /// module below one, in module order.
    pub test_modules: Vec<Id>,
    /// Kept `.rs` files no target reaches, in walk order.
    pub orphans: Vec<String>,
}

/// Builds the crate graph from the kept manifests and source files, both
/// in walk order.
///
/// - A manifest with a `[package]` table is a package. Its name is the
///   `name = "…"` under `[package]`, or else its directory's name. A second
///   package with a name already taken is ignored.
/// - A package's targets are `src/lib.rs` (lib), `src/main.rs` (bin), and
///   each `src/bin/*.rs` (bin), when kept.
/// - `mod x;` goes to `x.rs` or `x/mod.rs`. A crate root or `mod.rs` looks
///   beside itself; any other `a.rs` looks in `a/`. An inline `mod` is a
///   module row on its own file. `#[path]` counts only when it stays inside
///   the root and names a kept file.
/// - A file reached twice keeps its first module.
/// - A module is a test module when its declaration carries `#[cfg(test)]`
///   or its parent is a test module.
pub fn build(manifests: &[Manifest], files: &[SourceFile]) -> Graph {
    let kept: HashMap<&str, &SourceFile> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut graph = Graph::default();
    let mut names = HashSet::new();
    let mut next_target: Id = 1;
    for manifest in manifests {
        let Some(scan) = scan_manifest(&manifest.text) else {
            continue;
        };
        let dir = parent_dir(&manifest.path);
        let name = scan.name.unwrap_or_else(|| file_name(dir).to_owned());
        if name.is_empty() || !names.insert(name.clone()) {
            continue;
        }
        for (kind, target_name, root) in targets_of(dir, &name, files) {
            if kept.contains_key(root.as_str()) {
                graph.targets.push(Target {
                    id: next_target,
                    package: name.clone(),
                    name: target_name,
                    kind,
                    root,
                });
                next_target += 1;
            }
        }
        graph.packages.push(Package {
            name,
            version: scan.version,
            manifest_path: manifest.path.clone(),
        });
    }
    let mut tree = Tree {
        kept: &kept,
        modules: Vec::new(),
        claimed: HashSet::new(),
        cfg_test: HashSet::new(),
    };
    for target in &graph.targets {
        tree.target(target);
    }
    let claimed = tree.claimed;
    // A parent is always pushed before its children.
    let mut tests: HashSet<Id> = HashSet::new();
    for module in &tree.modules {
        if tree.cfg_test.contains(&module.id)
            || module.parent.is_some_and(|parent| tests.contains(&parent))
        {
            tests.insert(module.id);
            graph.test_modules.push(module.id);
        }
    }
    graph.modules = tree.modules;
    graph.orphans = files
        .iter()
        .filter(|file| !claimed.contains(file.path.as_str()))
        .map(|file| file.path.clone())
        .collect();
    graph
}

/// What a manifest's `[package]` table says.
struct ManifestScan {
    name: Option<String>,
    version: Option<String>,
}

/// Scans for a `[package]` table and its `name` and `version` strings.
fn scan_manifest(text: &str) -> Option<ManifestScan> {
    let mut in_package = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            if in_package {
                found.get_or_insert(ManifestScan {
                    name: None,
                    version: None,
                });
            }
            continue;
        }
        let Some(scan) = found.as_mut().filter(|_| in_package) else {
            continue;
        };
        if scan.name.is_none() {
            scan.name = string_value(line, "name");
        }
        if scan.version.is_none() {
            scan.version = string_value(line, "version");
        }
    }
    found
}

/// The string value of `key = "value"`, with either quote character.
fn string_value(line: &str, key: &str) -> Option<String> {
    let rest = line
        .strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = rest.chars().next().filter(|ch| *ch == '"' || *ch == '\'')?;
    let body = &rest[1..];
    let end = body.find(quote)?;
    Some(body[..end].to_owned())
}

/// The candidate targets of the package in `dir`.
fn targets_of(dir: &str, package: &str, files: &[SourceFile]) -> Vec<(TargetKind, String, String)> {
    let src = join(dir, "src");
    let mut out = vec![
        (TargetKind::Lib, package.to_owned(), join(&src, "lib.rs")),
        (TargetKind::Bin, package.to_owned(), join(&src, "main.rs")),
    ];
    let bin_dir = join(&src, "bin");
    for file in files {
        let Some(name) = file
            .path
            .strip_prefix(bin_dir.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.strip_suffix(".rs"))
        else {
            continue;
        };
        if !name.is_empty() && !name.contains('/') {
            out.push((TargetKind::Bin, name.to_owned(), file.path.clone()));
        }
    }
    out
}

/// The module rows built so far.
struct Tree<'a> {
    kept: &'a HashMap<&'a str, &'a SourceFile>,
    modules: Vec<Module>,
    claimed: HashSet<&'a str>,
    /// Modules whose own declaration carries `#[cfg(test)]`.
    cfg_test: HashSet<Id>,
}

impl<'a> Tree<'a> {
    fn target(&mut self, target: &Target) {
        let Some((root, file)) = self.kept.get_key_value(target.root.as_str()) else {
            return;
        };
        if !self.claimed.insert(*root) {
            return;
        }
        let root_id = self.push(target.id, None, "crate".to_owned(), root, String::new());
        let mut queue = VecDeque::from([(*file, root_id, true)]);
        while let Some((file, module_id, mod_rs)) = queue.pop_front() {
            self.file_decls(target.id, file, module_id, mod_rs, &mut queue);
        }
    }

    /// Adds the modules `file` declares. `mod_rs` is true for a crate root
    /// or a `mod.rs`, whose children sit beside it.
    fn file_decls(
        &mut self,
        target: Id,
        file: &'a SourceFile,
        file_module: Id,
        mod_rs: bool,
        queue: &mut VecDeque<(&'a SourceFile, Id, bool)>,
    ) {
        let mut scopes: HashMap<String, Id> = HashMap::from([(String::new(), file_module)]);
        for decl in &file.mod_decls {
            let Some(&parent) = scopes.get(&decl.scope) else {
                continue;
            };
            let path = format!("{}::{}", self.modules[index(parent)].path, decl.name);
            if decl.inline {
                let scope = join_scope(&decl.scope, &decl.name);
                let id = self.push(target, Some(parent), path, &file.path, scope.clone());
                if decl.cfg_test {
                    self.cfg_test.insert(id);
                }
                scopes.insert(scope, id);
                continue;
            }
            let Some(child) = self.child_file(file, mod_rs, decl) else {
                continue;
            };
            if !self.claimed.insert(child.path.as_str()) {
                continue;
            }
            let id = self.push(target, Some(parent), path, &child.path, String::new());
            if decl.cfg_test {
                self.cfg_test.insert(id);
            }
            // A `#[path]` file declares its children beside itself, as a
            // `mod.rs` does.
            let child_mod_rs = decl.path.is_some() || file_name(&child.path) == "mod.rs";
            queue.push_back((child, id, child_mod_rs));
        }
    }

    /// The kept file a non-inline `mod` declaration names, if any.
    fn child_file(
        &self,
        file: &SourceFile,
        mod_rs: bool,
        decl: &ModDecl,
    ) -> Option<&'a SourceFile> {
        let dir = parent_dir(&file.path);
        let base = if mod_rs {
            dir.to_owned()
        } else {
            join(dir, file_stem(&file.path))
        };
        let scoped = join(&base, &decl.scope.replace("::", "/"));
        let candidates = match &decl.path {
            Some(path) if path.starts_with('/') => Vec::new(),
            Some(path) => {
                let from = if decl.scope.is_empty() { dir } else { &scoped };
                normalize(&join(from, path)).into_iter().collect()
            }
            None => vec![
                join(&scoped, &format!("{}.rs", decl.name)),
                join(&scoped, &format!("{}/mod.rs", decl.name)),
            ],
        };
        candidates
            .iter()
            .find_map(|candidate| self.kept.get(candidate.as_str()).copied())
    }

    fn push(
        &mut self,
        target: Id,
        parent: Option<Id>,
        path: String,
        file: &str,
        scope: String,
    ) -> Id {
        let id = Id::try_from(self.modules.len()).map_or(Id::MAX, |len| len + 1);
        self.modules.push(Module {
            id,
            target,
            parent,
            path,
            file: file.to_owned(),
            scope,
        });
        id
    }
}

/// The position of module `id` in [`Tree::modules`]; ids count from 1.
fn index(id: Id) -> usize {
    usize::try_from(id - 1).unwrap_or(usize::MAX)
}

/// `path` with `.` and `..` folded, or none if it climbs out of the root.
pub(crate) fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

pub(crate) fn join(dir: &str, rest: &str) -> String {
    match (dir.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_owned(),
        (_, true) => dir.to_owned(),
        _ => format!("{dir}/{rest}"),
    }
}

fn join_scope(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}::{name}")
    }
}

pub(crate) fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    name.strip_suffix(".rs").unwrap_or(name)
}
