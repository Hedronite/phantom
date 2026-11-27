//! Language driver registry for multi-language extraction, walking,
//! fingerprinting, diffing, and graph building (SPEC-w17 G2b).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use phantom_fractal_lang::{
    Edge, Extract, ExtractError, FileModule, Graph, Id, Language, Module, ResolveCounts,
    ResolveInput, Resolver, SymbolKind, Target, TargetKind,
};
use phantom_fractal_lang_go::go_mod::{
    ExcludeReason, GoPackage, GoSourceFile, build_package_graph,
};
use phantom_fractal_lang_go::{GoLanguage, GoResolver};
use phantom_fractal_lang_rust::{
    RustLanguage, method_owners as rust_method_owners, resolve, resolve_graph,
};
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

/// Source-filtering metadata produced before persisting file extracts.
#[derive(Debug, Clone, Default)]
pub struct DriverFilter {
    /// If present, only walked paths in this set enter the store for this driver.
    pub kept_paths: Option<HashSet<String>>,
    /// Count of files excluded by language build constraints (for example Go build tags).
    pub build_excluded_count: u64,
    /// Encoded `<module_path>\t<ident1> <ident2>...` lines for build-excluded files.
    pub build_excluded_terms: String,
    /// Hashes of build-excluded files so build-tag edits trigger graph rebuilds.
    pub excluded_hashes: Vec<(String, [u8; 32])>,
}

/// Pluggable language backend pairing extraction, call resolution, manifests,
/// directory pruning, and package-graph construction.
pub trait LanguageDriver {
    fn language(&self) -> &dyn Language;
    fn resolver(&self) -> &dyn Resolver;
    fn manifest_names(&self) -> &'static [&'static str];
    fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool;
    fn build_graph(&self, manifests: &[ManifestFile], files: &[SourceFileExtract]) -> PackageGraph;

    fn needs_source_filter(&self) -> bool {
        false
    }

    fn filter_sources(
        &self,
        _manifests: &[ManifestFile],
        _sources: &[(String, String)],
    ) -> Option<DriverFilter> {
        None
    }

    fn build_graph_with_sources(
        &self,
        manifests: &[ManifestFile],
        files: &[SourceFileExtract],
        _sources: &HashMap<String, String>,
    ) -> PackageGraph {
        self.build_graph(manifests, files)
    }

    fn method_owners(&self, _input: &ResolveInput) -> Option<Vec<(Id, Id)>> {
        None
    }
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

    fn method_owners(&self, input: &ResolveInput) -> Option<Vec<(Id, Id)>> {
        Some(rust_method_owners(input))
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

enum DriverEntry<'a> {
    Borrowed(&'a dyn LanguageDriver),
    Owned(Box<dyn LanguageDriver + 'a>),
}

impl DriverEntry<'_> {
    fn as_driver(&self) -> &dyn LanguageDriver {
        match self {
            Self::Borrowed(driver) => *driver,
            Self::Owned(driver) => driver.as_ref(),
        }
    }
}

/// Registry of active [`LanguageDriver`]s for one walk/index/probe/diff pass.
pub struct Registry<'a> {
    drivers: Vec<DriverEntry<'a>>,
}

impl<'a> Registry<'a> {
    /// Creates a registry over borrowed `drivers`.
    pub fn new(drivers: Vec<&'a dyn LanguageDriver>) -> Self {
        Self {
            drivers: drivers.into_iter().map(DriverEntry::Borrowed).collect(),
        }
    }

    /// Creates a registry over owned `drivers`.
    pub fn from_owned(drivers: Vec<Box<dyn LanguageDriver + 'a>>) -> Self {
        Self {
            drivers: drivers.into_iter().map(DriverEntry::Owned).collect(),
        }
    }

    pub(crate) fn drivers(&self) -> Vec<&dyn LanguageDriver> {
        self.drivers.iter().map(DriverEntry::as_driver).collect()
    }

    /// Finds the driver claiming `path`'s extension.
    pub fn language_for(&self, path: &str) -> Option<&dyn LanguageDriver> {
        let ext = Path::new(path).extension().and_then(|ext| ext.to_str())?;
        self.drivers
            .iter()
            .map(DriverEntry::as_driver)
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
            .any(|driver| driver.as_driver().manifest_names().contains(&name))
    }

    /// Collects static prune predicates for all registered drivers.
    pub fn prune_rules(&self) -> Vec<fn(&Path, &str, usize) -> bool> {
        self.drivers
            .iter()
            .map(|d| d.as_driver().prune_rule())
            .collect()
    }

    /// True when `.git` or any registered driver prunes directory `name`.
    pub fn should_prune_dir(&self, parent: &Path, name: &str, depth: usize) -> bool {
        name == ".git"
            || self
                .drivers
                .iter()
                .any(|driver| (driver.as_driver().prune_rule())(parent, name, depth))
    }
}

/// Default Go [`LanguageDriver`] backed by `phantom-fractal-lang-go`.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoDriver {
    lang: GoLanguage,
    resolver: GoResolver,
}

impl GoDriver {
    /// Creates a new Go language driver.
    pub fn new() -> Self {
        Self {
            lang: GoLanguage::new(),
            resolver: GoResolver::new(),
        }
    }
}

/// True when directory `name` should be skipped by Go walk rules inside a Go tree.
/// Leaves `testdata` walked so `diff` catches unindexed edits in `testdata/`.
pub(crate) fn is_go_dir_pruned(parent: &Path, name: &str, depth: usize) -> bool {
    is_go_pruned_name(name) && has_go_manifest_ancestor(parent, depth)
}

fn is_go_pruned_name(name: &str) -> bool {
    name == "vendor"
        || ((name.starts_with('.') || name.starts_with('_')) && name != "." && name != "..")
}

fn has_go_manifest_ancestor(parent: &Path, depth: usize) -> bool {
    let mut cur = Some(parent);
    for _ in 0..depth {
        let Some(dir) = cur else {
            break;
        };
        if dir.join("go.mod").is_file() || dir.join("go.work").is_file() {
            return true;
        }
        cur = dir.parent();
    }
    false
}

impl LanguageDriver for GoDriver {
    fn language(&self) -> &dyn Language {
        &self.lang
    }

    fn resolver(&self) -> &dyn Resolver {
        &self.resolver
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["go.mod", "go.work"]
    }

    fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool {
        is_go_dir_pruned
    }

    fn needs_source_filter(&self) -> bool {
        true
    }

    fn filter_sources(
        &self,
        manifests: &[ManifestFile],
        sources: &[(String, String)],
    ) -> Option<DriverFilter> {
        let mut go_files = collect_go_manifest_inputs(manifests);
        for (path, content) in sources {
            go_files.push(GoSourceFile { path, content });
        }
        let pkg_graph = build_package_graph(&go_files);
        let kept_paths: HashSet<String> = pkg_graph.file_to_package.keys().cloned().collect();
        let src_map: HashMap<&str, &str> = sources
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let mut excluded_terms = Vec::new();
        let mut excluded_hashes = Vec::new();
        let mut build_excluded_count = 0u64;
        for ex in &pkg_graph.excluded {
            if ex.reason != ExcludeReason::BuildConstraint {
                continue;
            }
            build_excluded_count += 1;
            if let Some(&content) = src_map.get(ex.path.as_str()) {
                excluded_hashes.push((
                    ex.path.clone(),
                    *blake3::hash(content.as_bytes()).as_bytes(),
                ));
                let mod_path = enclosing_go_module_path(&ex.path, &pkg_graph.modules);
                let idents = collect_go_identifiers(content);
                excluded_terms.push(format!("{mod_path}\t{idents}"));
            }
        }
        Some(DriverFilter {
            kept_paths: Some(kept_paths),
            build_excluded_count,
            build_excluded_terms: excluded_terms.join("\n"),
            excluded_hashes,
        })
    }

    fn build_graph(&self, manifests: &[ManifestFile], files: &[SourceFileExtract]) -> PackageGraph {
        let empty = HashMap::new();
        self.build_graph_with_sources(manifests, files, &empty)
    }

    fn build_graph_with_sources(
        &self,
        manifests: &[ManifestFile],
        files: &[SourceFileExtract],
        sources: &HashMap<String, String>,
    ) -> PackageGraph {
        let mut go_files = collect_go_manifest_inputs(manifests);
        for f in files {
            let content = sources.get(&f.path).map_or("", String::as_str);
            go_files.push(GoSourceFile {
                path: &f.path,
                content,
            });
        }
        let built = build_package_graph(&go_files);
        convert_go_package_graph(&built, files)
    }

    fn method_owners(&self, input: &ResolveInput) -> Option<Vec<(Id, Id)>> {
        let mut types_by_mod: HashMap<(Id, &str), Id> = HashMap::new();
        for sym in &input.symbols {
            if matches!(
                sym.kind,
                SymbolKind::Struct | SymbolKind::Interface | SymbolKind::TypeAlias
            ) && let Some(mid) = sym.module
            {
                types_by_mod.insert((mid, sym.name.as_str()), sym.id);
            }
        }
        Some(
            input
                .symbols
                .iter()
                .filter(|s| s.kind == SymbolKind::Method)
                .filter_map(|m| {
                    let (mid, owner) = (m.module?, m.owner.as_deref()?);
                    types_by_mod.get(&(mid, owner)).map(|&oid| (m.id, oid))
                })
                .collect(),
        )
    }
}

fn collect_go_manifest_inputs(manifests: &[ManifestFile]) -> Vec<GoSourceFile<'_>> {
    manifests
        .iter()
        .filter(|m| is_named_file(&m.path, "go.mod") || is_named_file(&m.path, "go.work"))
        .map(|m| GoSourceFile {
            path: &m.path,
            content: &m.text,
        })
        .collect()
}

fn enclosing_go_module_path<'a>(
    file_path: &str,
    modules: &'a [phantom_fractal_lang_go::go_mod::GoModuleSpec],
) -> &'a str {
    let dir = file_path.rsplit_once('/').map_or("", |(d, _)| d);
    modules
        .iter()
        .filter(|m| {
            m.in_workspace
                && (m.dir.is_empty()
                    || dir == m.dir
                    || dir.strip_prefix(&m.dir).is_some_and(|r| r.starts_with('/')))
        })
        .max_by_key(|m| m.dir.len())
        .map_or("", |m| m.module_path.as_str())
}

fn collect_go_identifiers(content: &str) -> String {
    let set: BTreeSet<&str> = content
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|tok| !tok.is_empty() && !tok.as_bytes()[0].is_ascii_digit())
        .collect();
    set.into_iter().collect::<Vec<_>>().join(" ")
}

fn convert_go_package_graph(
    built: &phantom_fractal_lang_go::go_mod::PackageGraph,
    files: &[SourceFileExtract],
) -> PackageGraph {
    let (mut packages, names_by_dir) = build_go_module_packages(built);
    let mut graph = PackageGraph::default();
    let needs_rootless = populate_go_targets_and_modules(built, &names_by_dir, &mut graph);
    if needs_rootless {
        packages.push(Package {
            name: "go-rootless".to_owned(),
            version: None,
            manifest_path: String::new(),
        });
    }
    graph.packages = packages;
    graph.orphans = files
        .iter()
        .filter(|f| !built.file_to_package.contains_key(&f.path))
        .count();
    graph
}

fn build_go_module_packages(
    built: &phantom_fractal_lang_go::go_mod::PackageGraph,
) -> (Vec<Package>, HashMap<String, String>) {
    let mut packages = Vec::new();
    let mut names_by_dir = HashMap::new();
    for m in built.modules.iter().filter(|m| m.in_workspace) {
        let name = if m.module_path.is_empty() {
            "go-root".to_owned()
        } else {
            m.module_path.clone()
        };
        let manifest_path = if m.dir.is_empty() {
            "go.mod".to_owned()
        } else {
            format!("{}/go.mod", m.dir)
        };
        names_by_dir.insert(m.dir.clone(), name.clone());
        packages.push(Package {
            name,
            version: None,
            manifest_path,
        });
    }
    (packages, names_by_dir)
}

fn populate_go_targets_and_modules(
    built: &phantom_fractal_lang_go::go_mod::PackageGraph,
    names_by_dir: &HashMap<String, String>,
    graph: &mut PackageGraph,
) -> bool {
    let mut needs_rootless = false;
    for (idx, pkg) in built.packages.iter().enumerate() {
        let pkg_name = match names_by_dir.get(&pkg.module_dir) {
            Some(name) => name.clone(),
            None => {
                needs_rootless = true;
                "go-rootless".to_owned()
            }
        };
        push_go_package_rows((idx + 1) as Id, &pkg_name, pkg, graph);
    }
    needs_rootless
}

fn push_go_package_rows(id: Id, pkg_name: &str, pkg: &GoPackage, graph: &mut PackageGraph) {
    let kind = if pkg.is_external_test {
        TargetKind::Test
    } else if pkg.is_main {
        TargetKind::Bin
    } else {
        TargetKind::Lib
    };
    graph.targets.push(Target {
        id,
        package: pkg_name.to_owned(),
        name: pkg.package_name.clone(),
        kind,
        root: pkg.files.first().cloned().unwrap_or_default(),
    });
    graph.modules.push(Module {
        id,
        target: id,
        parent: None,
        path: pkg.import_path.clone(),
        file: String::new(),
        scope: String::new(),
    });
    if pkg.is_external_test {
        graph.test_modules.push(id);
    }
    for file in &pkg.files {
        graph.file_modules.push(FileModule {
            file: file.clone(),
            module: id,
        });
    }
}

/// Constructs the single default [`Registry`] for CLI and library callers.
pub fn default_registry() -> Result<Registry<'static>, ExtractError> {
    Ok(Registry::from_owned(vec![
        Box::new(RustDriver::owned()?),
        Box::new(GoDriver::new()),
    ]))
}

/// Lightweight Cargo manifest filter used by internal helpers.
pub(crate) struct RustFilter;

impl RustFilter {
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

pub(crate) fn has_ext(path: &str, exts: &[&str]) -> bool {
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

        assert!(RustFilter::is_manifest("Cargo.toml"));
        assert!(RustFilter::is_manifest("crates/a/Cargo.toml"));
        assert!(!RustFilter::is_manifest("crates/a/not_Cargo.toml"));
        assert!(!RustFilter::is_manifest("README.md"));
        assert!(is_named_file("Cargo.toml", "Cargo.toml"));
        assert!(!is_named_file("fooCargo.toml", "Cargo.toml"));

        let go = GoDriver::new();
        let src = SourceFileExtract {
            path: "loose.go".to_owned(),
            extract: phantom_fractal_lang::Extract::default(),
        };
        let mut sources = HashMap::new();
        sources.insert(
            "loose.go".to_owned(),
            "package loose
func Loose() {}
"
            .to_owned(),
        );
        let g = go.build_graph_with_sources(&[], &[src], &sources);
        assert_eq!(g.packages[0].name, "go-rootless");
        assert!(g.targets.len() == 1);
        let empty_mod = ManifestFile {
            path: "sub/go.mod".to_owned(),
            text: "go 1.26
"
            .to_owned(),
        };
        let g2 = go.build_graph(&[empty_mod], &[]);
        assert_eq!(g2.packages[0].manifest_path, "sub/go.mod");
        assert_eq!(g2.packages[0].name, "go-root");
        assert!(!is_go_dir_pruned(Path::new("."), "vendor", 0));
    }

    #[test]
    fn language_driver_defaults_and_go_helpers_cover_edge_cases() {
        use phantom_fractal_lang_go::go_mod::GoModuleSpec;
        use phantom_fractal_lang_rust::RustLanguage;

        struct MinimalDriver(RustLanguage);
        impl LanguageDriver for MinimalDriver {
            fn language(&self) -> &dyn Language {
                &self.0
            }
            fn resolver(&self) -> &dyn Resolver {
                &RustResolver
            }
            fn manifest_names(&self) -> &'static [&'static str] {
                &[]
            }
            fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool {
                |_, _, _| false
            }
            fn build_graph(
                &self,
                _manifests: &[ManifestFile],
                _files: &[SourceFileExtract],
            ) -> PackageGraph {
                PackageGraph::default()
            }
        }
        let minimal = MinimalDriver(RustLanguage::new().unwrap());
        assert!(!minimal.needs_source_filter());
        assert!(minimal.filter_sources(&[], &[]).is_none());
        assert!(minimal.method_owners(&ResolveInput::default()).is_none());

        let go = GoDriver::new();
        assert_eq!(go.manifest_names(), &["go.mod", "go.work"]);

        // Go directory prune rules inside go.mod, go.work, and non-Go dir.
        let tmp = std::env::temp_dir().join(format!("phantom-go-prune-{}", std::process::id()));
        let mod_dir = tmp.join("mod_tree");
        let work_dir = tmp.join("work_tree");
        let plain_dir = tmp.join("plain_tree");
        std::fs::create_dir_all(mod_dir.join("sub")).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();
        std::fs::create_dir_all(&plain_dir).unwrap();
        std::fs::write(mod_dir.join("go.mod"), "module example.com/m\ngo 1.26\n").unwrap();
        std::fs::write(work_dir.join("go.work"), "go 1.26\n").unwrap();
        assert!(is_go_dir_pruned(&mod_dir.join("sub"), "vendor", 2));
        assert!(is_go_dir_pruned(&mod_dir, ".git", 1));
        assert!(is_go_dir_pruned(&mod_dir, "_gen", 1));
        assert!(!is_go_dir_pruned(&mod_dir, ".", 1));
        assert!(!is_go_dir_pruned(&mod_dir, "..", 1));
        assert!(!is_go_dir_pruned(&mod_dir, "testdata", 1));
        assert!(!is_go_dir_pruned(&mod_dir, "pkg", 1));
        assert!(is_go_dir_pruned(&work_dir, "vendor", 1));
        assert!(!is_go_dir_pruned(&plain_dir, "vendor", 1));
        let _ = std::fs::remove_dir_all(&tmp);

        // enclosing_go_module_path and collect_go_identifiers.
        let specs = vec![
            GoModuleSpec {
                dir: String::new(),
                module_path: "example.com/root".to_owned(),
                go_minor: 26,
                in_workspace: true,
            },
            GoModuleSpec {
                dir: "sub".to_owned(),
                module_path: "example.com/sub".to_owned(),
                go_minor: 26,
                in_workspace: true,
            },
            GoModuleSpec {
                dir: "ext".to_owned(),
                module_path: "example.com/ext".to_owned(),
                go_minor: 26,
                in_workspace: false,
            },
        ];
        assert_eq!(
            enclosing_go_module_path("main.go", &specs),
            "example.com/root"
        );
        assert_eq!(
            enclosing_go_module_path("sub/a.go", &specs),
            "example.com/sub"
        );
        assert_eq!(
            enclosing_go_module_path("sub/deep/b.go", &specs),
            "example.com/sub"
        );
        assert_eq!(
            enclosing_go_module_path("sub2/c.go", &specs),
            "example.com/root"
        );
        assert_eq!(enclosing_go_module_path("ext/x.go", &specs[2..]), "");
        assert_eq!(
            collect_go_identifiers("package p\nvar 9skip = _under + foo_2\n"),
            "_under foo_2 p package var"
        );

        // filter_sources builds counts (>1), hashes, and terms for build-excluded files.
        let man = vec![ManifestFile {
            path: "go.mod".to_owned(),
            text: "module example.com/test\ngo 1.26\n".to_owned(),
        }];
        let filtered = go.filter_sources(
            &man,
            &[
                ("a.go".to_owned(), "package p\nfunc Keep() {}\n".to_owned()),
                (
                    "a_darwin.go".to_owned(),
                    "package p\nfunc Dar() {}\n".to_owned(),
                ),
                (
                    "tag.go".to_owned(),
                    "//go:build custom\n\npackage p\nfunc TagFn() {}\n".to_owned(),
                ),
            ],
        );
        let filtered = filtered.expect("go filter");
        assert_eq!(filtered.build_excluded_count, 2);
        assert_eq!(filtered.excluded_hashes.len(), 2);
        assert!(filtered.build_excluded_terms.contains("example.com/test\t"));
        assert!(filtered.build_excluded_terms.contains("Dar"));
        assert!(filtered.build_excluded_terms.contains("TagFn"));

        // method_owners links Method symbols to matching Struct/Interface in same module.
        let owner_input = ResolveInput {
            symbols: vec![
                Symbol {
                    id: 10,
                    kind: SymbolKind::Struct,
                    name: "Srv".to_owned(),
                    owner: None,
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(1),
                    file: "a.go".to_owned(),
                    signature: None,
                },
                Symbol {
                    id: 11,
                    kind: SymbolKind::Method,
                    name: "Run".to_owned(),
                    owner: Some("Srv".to_owned()),
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(1),
                    file: "a.go".to_owned(),
                    signature: None,
                },
                Symbol {
                    id: 12,
                    kind: SymbolKind::Function,
                    name: "Top".to_owned(),
                    owner: Some("Srv".to_owned()),
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(1),
                    file: "a.go".to_owned(),
                    signature: None,
                },
            ],
            ..ResolveInput::default()
        };
        assert_eq!(go.method_owners(&owner_input), Some(vec![(11, 10)]));

        // Multi-package convert_go_package_graph gives 1-based target/module ids and exact orphan count.
        let g_multi = go.build_graph_with_sources(
            &man,
            &[
                SourceFileExtract {
                    path: "a.go".to_owned(),
                    extract: phantom_fractal_lang::Extract::default(),
                },
                SourceFileExtract {
                    path: "sub/b.go".to_owned(),
                    extract: phantom_fractal_lang::Extract::default(),
                },
            ],
            &HashMap::from([
                ("a.go".to_owned(), "package p\nfunc F() {}\n".to_owned()),
                (
                    "sub/b.go".to_owned(),
                    "package sub\nfunc G() {}\n".to_owned(),
                ),
            ]),
        );
        assert_eq!(g_multi.orphans, 0);
        assert_eq!(g_multi.targets.len(), 2);
        assert_eq!(g_multi.targets[0].id, 1);
        assert_eq!(g_multi.targets[1].id, 2);
        assert_eq!(g_multi.modules[0].id, 1);
        assert_eq!(g_multi.modules[1].id, 2);
    }
}
