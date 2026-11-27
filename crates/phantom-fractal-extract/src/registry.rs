//! Language driver registry for multi-language extraction, walking,
//! fingerprinting, diffing, and graph building (SPEC-w17 G2b).

use std::collections::HashSet;
use std::path::Path;

use phantom_fractal_lang::{
    Edge, Extract, ExtractError, FileModule, Graph, Id, Language, Module, ResolveCounts,
    ResolveInput, Resolver, Target,
};
use phantom_fractal_lang_rust::{RustLanguage, resolve, resolve_graph};
use phantom_fractal_store::Package;

use crate::graph::{self, Manifest, SourceFile};

/// One extracted source file paired with its repo-relative path.
#[derive(Debug, Clone)]
pub struct SourceFileExtract {
    pub path: String,
    pub extract: Extract,
}

/// One walked manifest file with its UTF-8 text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub text: String,
}

/// Package, target, module, and file-membership graph built by one
/// [`LanguageDriver`].
#[derive(Debug, Clone, Default)]
pub struct PackageGraph {
    pub packages: Vec<Package>,
    pub targets: Vec<Target>,
    pub modules: Vec<Module>,
    pub file_modules: Vec<FileModule>,
    pub test_modules: Vec<Id>,
    pub orphans: usize,
}

/// Pluggable language backend pairing extraction, call resolution, manifests,
/// directory pruning, and package-graph construction.
pub trait LanguageDriver {
    fn language(&self) -> &dyn Language;
    fn resolver(&self) -> &dyn Resolver;
    fn manifest_names(&self) -> &'static [&'static str];
    fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool;
    fn build_graph(&self, manifests: &[ManifestFile], files: &[SourceFileExtract]) -> PackageGraph;
}

/// [`Resolver`] implementation backed by `phantom-fractal-lang-rust`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustResolver;

impl Resolver for RustResolver {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn resolve(&self, input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
        resolve(input)
    }

    fn resolve_graph(&self, input: &ResolveInput) -> Graph {
        resolve_graph(input)
    }
}

enum RustLangRef<'a> {
    Owned(RustLanguage),
    Borrowed(&'a RustLanguage),
}

impl RustLangRef<'_> {
    fn as_lang(&self) -> &RustLanguage {
        match self {
            Self::Owned(lang) => lang,
            Self::Borrowed(lang) => lang,
        }
    }
}

/// Default Rust [`LanguageDriver`].
pub struct RustDriver<'a> {
    lang: RustLangRef<'a>,
    resolver: RustResolver,
}

impl<'a> RustDriver<'a> {
    /// Wraps a caller-provided [`RustLanguage`] without recompiling queries.
    pub fn new(language: &'a RustLanguage) -> Self {
        Self {
            lang: RustLangRef::Borrowed(language),
            resolver: RustResolver,
        }
    }

    /// Compiles and owns a fresh [`RustLanguage`].
    pub fn owned() -> Result<RustDriver<'static>, ExtractError> {
        Ok(RustDriver {
            lang: RustLangRef::Owned(RustLanguage::new()?),
            resolver: RustResolver,
        })
    }
}

/// True when `target` is pruned under Cargo rules (at root depth or beside
/// `Cargo.toml`).
pub(crate) fn is_rust_target_pruned(parent: &Path, name: &str, depth: usize) -> bool {
    name == "target" && (depth == 1 || parent.join("Cargo.toml").is_file())
}

impl LanguageDriver for RustDriver<'_> {
    fn language(&self) -> &dyn Language {
        self.lang.as_lang()
    }

    fn resolver(&self) -> &dyn Resolver {
        &self.resolver
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["Cargo.toml"]
    }

    fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool {
        is_rust_target_pruned
    }

    fn build_graph(&self, manifests: &[ManifestFile], files: &[SourceFileExtract]) -> PackageGraph {
        let rust_manifests: Vec<Manifest> = manifests
            .iter()
            .filter(|m| is_named_file(&m.path, "Cargo.toml"))
            .map(|m| Manifest {
                path: m.path.clone(),
                text: m.text.clone(),
            })
            .collect();
        let source_files: Vec<SourceFile> = files
            .iter()
            .map(|f| SourceFile {
                path: f.path.clone(),
                mod_decls: f.extract.mod_decls.clone(),
            })
            .collect();
        let built = graph::build(&rust_manifests, &source_files);
        PackageGraph {
            packages: built.packages,
            targets: built.targets,
            modules: built.modules,
            file_modules: Vec::new(),
            test_modules: built.test_modules,
            orphans: built.orphans.len(),
        }
    }
}

/// Registry of active [`LanguageDriver`]s for one walk/index/probe/diff pass.
pub struct Registry<'a> {
    drivers: Vec<&'a dyn LanguageDriver>,
}

impl<'a> Registry<'a> {
    /// Creates a registry over `drivers`.
    pub fn new(drivers: Vec<&'a dyn LanguageDriver>) -> Self {
        Self { drivers }
    }

    pub(crate) fn drivers(&self) -> &[&'a dyn LanguageDriver] {
        &self.drivers
    }

    /// Finds the driver claiming `path`'s extension.
    pub fn language_for(&self, path: &str) -> Option<&dyn LanguageDriver> {
        let ext = Path::new(path).extension().and_then(|ext| ext.to_str())?;
        self.drivers
            .iter()
            .copied()
            .find(|driver| driver.language().extensions().contains(&ext))
    }

    /// True when any registered driver claims `path`'s extension.
    pub fn is_supported(&self, path: &str) -> bool {
        self.language_for(path).is_some()
    }

    /// True when `path`'s filename matches any registered driver's manifest names.
    pub fn is_manifest(&self, path: &str) -> bool {
        let Some(name) = Path::new(path).file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        self.drivers
            .iter()
            .any(|driver| driver.manifest_names().contains(&name))
    }

    /// Collects static prune predicates for all registered drivers.
    pub fn prune_rules(&self) -> Vec<fn(&Path, &str, usize) -> bool> {
        self.drivers.iter().map(|d| d.prune_rule()).collect()
    }

    /// True when `.git` or any registered driver prunes directory `name`.
    pub fn should_prune_dir(&self, parent: &Path, name: &str, depth: usize) -> bool {
        name == ".git"
            || self
                .drivers
                .iter()
                .any(|driver| (driver.prune_rule())(parent, name, depth))
    }
}

/// Lightweight path/prune filter used by default [`crate::probe`] and
/// [`crate::walk`] without compiling tree-sitter queries.
pub(crate) struct RustFilter;

impl RustFilter {
    pub(crate) fn is_supported(path: &str) -> bool {
        Path::new(path).extension().is_some_and(|ext| ext == "rs")
    }

    pub(crate) fn is_manifest(path: &str) -> bool {
        is_named_file(path, "Cargo.toml")
    }
}

pub(crate) fn is_named_file(path: &str, filename: &str) -> bool {
    path == filename
        || path
            .strip_suffix(filename)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

/// Filters `input` to rows belonging to `exts` (and `target_ids`).
pub(crate) fn scope_input(
    input: &ResolveInput,
    exts: &[&str],
    target_ids: &HashSet<Id>,
) -> ResolveInput {
    let modules: Vec<Module> = input
        .modules
        .iter()
        .filter(|m| target_ids.contains(&m.target))
        .cloned()
        .collect();
    let targets: Vec<Target> = input
        .targets
        .iter()
        .filter(|t| target_ids.contains(&t.id))
        .cloned()
        .collect();
    let symbols = input
        .symbols
        .iter()
        .filter(|s| has_ext(&s.file, exts))
        .cloned()
        .collect();
    let uses = input
        .uses
        .iter()
        .filter(|u| has_ext(&u.file, exts))
        .cloned()
        .collect();
    let refs = input
        .refs
        .iter()
        .filter(|r| has_ext(&r.file, exts))
        .cloned()
        .collect();
    ResolveInput {
        targets,
        modules,
        symbols,
        uses,
        refs,
        deps: input.deps.clone(),
    }
}

fn has_ext(path: &str, exts: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.contains(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use phantom_fractal_lang::RefKind;
    use phantom_fractal_lang::resolve::{CallRef, Module, Symbol, SymbolKind, Target, TargetKind};

    #[test]
    fn rust_resolver_resolve_delegates_cleanly() {
        let input = ResolveInput {
            targets: vec![Target {
                id: 1,
                package: "p".to_owned(),
                name: "p".to_owned(),
                kind: TargetKind::Lib,
                root: "src/lib.rs".to_owned(),
            }],
            modules: vec![Module {
                id: 1,
                target: 1,
                parent: None,
                path: "crate".to_owned(),
                file: "src/lib.rs".to_owned(),
                scope: String::new(),
            }],
            symbols: vec![
                Symbol {
                    id: 1,
                    kind: SymbolKind::Function,
                    name: "a".to_owned(),
                    owner: None,
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(1),
                    file: "src/lib.rs".to_owned(),
                    signature: None,
                },
                Symbol {
                    id: 2,
                    kind: SymbolKind::Function,
                    name: "b".to_owned(),
                    owner: None,
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(1),
                    file: "src/lib.rs".to_owned(),
                    signature: None,
                },
            ],
            uses: Vec::new(),
            refs: vec![CallRef {
                id: 10,
                from: Some(1),
                module: Some(1),
                scope: String::new(),
                file: "src/lib.rs".to_owned(),
                kind: RefKind::Call,
                text: "b".to_owned(),
                receiver: None,
                receiver_type: None,
                shadowed: false,
            }],
            deps: Vec::new(),
        };
        let (edges, counts) = RustResolver.resolve(&input);
        assert_eq!(edges.len(), 1);
        assert_eq!(counts.edges, 1);
    }
}
