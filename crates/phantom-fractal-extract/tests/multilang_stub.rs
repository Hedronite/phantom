//! End-to-end G2b multi-language verification using a stub `.toy` + `toy.mod`
//! second language driver BEFORE Go exists (QA §7.2, RUBRIC G2b).

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{TempDir, copy_tree};
use phantom_fractal_extract::{
    LanguageDriver, ManifestFile, PackageGraph, Registry, RustDriver, SourceFileExtract, diff_with,
    index, index_with, probe_with, stored_graph_with, walk_with,
};
use phantom_fractal_git::{Base, FileChange};
use phantom_fractal_lang::{
    Caller, Edge, Extract, ExtractError, FileModule, Import, Item, ItemKind, Language, Module,
    Reason, RefKind, Reference, ResolveCounts, ResolveInput, Resolver, Target, TargetKind,
    Visibility,
};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{Package, Store};

/// Line-oriented `.toy` stub language:
/// - `pkg <dir>` declares package directory
/// - `fn <name>` (or `pub fn <name>`) declares a top-level function
/// - `method <Owner>.<name>` declares a method
/// - `call <from> -> <callee>` records a call ref
/// - `value_ref <from> -> <target>` records a non-call `ValueRef`
/// - `// SYNTAX_ERROR` triggers `has_parse_errors() == true`
struct ToyLanguage;

impl Language for ToyLanguage {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["toy"]
    }

    fn qualifier_separator(&self) -> &'static str {
        "."
    }

    fn has_parse_errors(&self, source: &str) -> bool {
        source.contains("SYNTAX_ERROR")
    }

    fn extract(&self, source: &str) -> Result<Extract, ExtractError> {
        let mut out = Extract::default();
        for (idx, raw) in source.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw.trim();
            if let Some(rest) = trimmed.strip_prefix("pub fn ") {
                let name = rest.split_whitespace().next().unwrap_or(rest);
                out.items
                    .push(toy_item(ItemKind::Function, name, Visibility::Public, line));
                out.item_scopes.push(String::new());
                out.item_owners.push(None);
            } else if let Some(rest) = trimmed.strip_prefix("fn ") {
                let name = rest.split_whitespace().next().unwrap_or(rest);
                out.items.push(toy_item(
                    ItemKind::Function,
                    name,
                    Visibility::Private,
                    line,
                ));
                out.item_scopes.push(String::new());
                out.item_owners.push(None);
            } else if let Some(rest) = trimmed.strip_prefix("method ") {
                if let Some((owner, name)) = rest.split_once('.') {
                    out.items
                        .push(toy_item(ItemKind::Function, name, Visibility::Public, line));
                    out.item_scopes.push(String::new());
                    out.item_owners.push(Some(owner.to_owned()));
                }
            } else if let Some(rest) = trimmed.strip_prefix("import ") {
                out.imports.push(Import {
                    path: rest.trim().to_owned(),
                    alias: None,
                    glob: false,
                    start_line: line,
                    visibility: Visibility::Private,
                });
                out.import_scopes.push(String::new());
            } else if let Some(rest) = trimmed.strip_prefix("call ") {
                if let Some((from, callee)) = rest.split_once("->") {
                    out.refs
                        .push(toy_ref(RefKind::Call, callee.trim(), from.trim(), line));
                }
            } else if let Some(rest) = trimmed.strip_prefix("value_ref ")
                && let Some((from, target)) = rest.split_once("->")
            {
                out.refs
                    .push(toy_ref(RefKind::ValueRef, target.trim(), from.trim(), line));
            }
        }
        Ok(out)
    }
}

fn toy_item(kind: ItemKind, name: &str, visibility: Visibility, line: u32) -> Item {
    Item {
        kind,
        name: name.to_owned(),
        visibility,
        start_line: line,
        end_line: line,
        signature: format!("{} {name}", kind.as_str()),
        doc_summary: None,
        is_test: false,
    }
}

fn toy_ref(kind: RefKind, text: &str, caller: &str, line: u32) -> Reference {
    Reference {
        kind,
        text: text.to_owned(),
        last_segment: text.rsplit('.').next().unwrap_or(text).to_owned(),
        line,
        col: 1,
        scope: String::new(),
        from: Some(Caller {
            name: caller.to_owned(),
            owner: None,
            start_line: 1,
        }),
        receiver: None,
        receiver_type: None,
        shadowed: false,
    }
}

struct ToyResolver;

impl Resolver for ToyResolver {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn resolve(&self, input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
        let mut by_name: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
        for sym in &input.symbols {
            by_name.entry(sym.name.as_str()).or_default().push(sym.id);
        }
        let mut calls = Vec::new();
        let mut counts = ResolveCounts::default();
        for r in &input.refs {
            let last = r.text.rsplit('.').next().unwrap_or(&r.text);
            let Some(from) = r.from else {
                counts.no_source += 1;
                continue;
            };
            if let Some(targets) = by_name.get(last)
                && targets.len() == 1
            {
                calls.push(Edge {
                    ref_id: r.id,
                    source: from,
                    target: targets[0],
                    confidence: 1.0,
                    reason: Reason::PackageImport,
                });
                counts.edges += 1;
            } else {
                counts.unresolved += 1;
            }
        }
        (calls, counts)
    }
}

fn is_toy_pruned(_parent: &Path, name: &str, _depth: usize) -> bool {
    matches!(name, "toy_vendor" | "testdata")
}

struct ToyDriver {
    lang: ToyLanguage,
    resolver: ToyResolver,
}

impl ToyDriver {
    fn new() -> Self {
        Self {
            lang: ToyLanguage,
            resolver: ToyResolver,
        }
    }
}

impl LanguageDriver for ToyDriver {
    fn language(&self) -> &dyn Language {
        &self.lang
    }

    fn resolver(&self) -> &dyn Resolver {
        &self.resolver
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["toy.mod"]
    }

    fn prune_rule(&self) -> fn(&Path, &str, usize) -> bool {
        is_toy_pruned
    }

    fn build_graph(&self, manifests: &[ManifestFile], files: &[SourceFileExtract]) -> PackageGraph {
        let mod_name = manifests
            .iter()
            .find(|m| m.path == "toy.mod" || m.path.ends_with("/toy.mod"))
            .and_then(|m| m.text.lines().next())
            .unwrap_or("example.com/toy")
            .trim()
            .to_owned();
        let manifest_path = manifests
            .iter()
            .find(|m| m.path == "toy.mod" || m.path.ends_with("/toy.mod"))
            .map_or_else(|| "toy.mod".to_owned(), |m| m.path.clone());

        let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in files {
            let dir = Path::new(&f.path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_owned();
            dirs.entry(dir).or_default().push(f.path.clone());
        }
        if dirs.is_empty() {
            return PackageGraph {
                packages: vec![Package {
                    name: mod_name,
                    version: None,
                    manifest_path,
                }],
                ..PackageGraph::default()
            };
        }
        let mut targets = Vec::new();
        let mut modules = Vec::new();
        let mut file_modules = Vec::new();
        let mut test_modules = Vec::new();
        for (idx, (dir, dir_files)) in dirs.into_iter().enumerate() {
            let id = (idx as i64) + 1;
            let pkg_short = dir.rsplit('/').next().unwrap_or("main");
            let mod_path = if dir.is_empty() {
                mod_name.clone()
            } else {
                format!("{mod_name}/{dir}")
            };
            targets.push(Target {
                id,
                package: mod_name.clone(),
                name: pkg_short.to_owned(),
                kind: if pkg_short == "cmd" {
                    TargetKind::Bin
                } else {
                    TargetKind::Lib
                },
                root: String::new(),
            });
            modules.push(Module {
                id,
                target: id,
                parent: None,
                path: mod_path,
                file: String::new(),
                scope: String::new(),
            });
            for f in dir_files {
                if f.ends_with("_test.toy") && !test_modules.contains(&id) {
                    test_modules.push(id);
                }
                file_modules.push(FileModule {
                    file: f,
                    module: id,
                });
            }
        }
        PackageGraph {
            packages: vec![Package {
                name: mod_name,
                version: None,
                manifest_path,
            }],
            targets,
            modules,
            file_modules,
            test_modules,
            orphans: 0,
        }
    }
}

fn git(root: &Path, home: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn pure_rust_tree_is_identical_with_toy_driver_registered() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/workspace");
    let tree1 = TempDir::new("d3-rust");
    let tree2 = TempDir::new("d3-multi");
    copy_tree(&fixture, tree1.path());
    copy_tree(&fixture, tree2.path());
    let db1 = TempDir::new("d3-db1").path().join("g.db");
    let db2 = TempDir::new("d3-db2").path().join("g.db");
    let mut s1 = Store::open(&db1).unwrap();
    let mut s2 = Store::open(&db2).unwrap();

    let rust = RustLanguage::new().unwrap();
    let r1 = index(tree1.path(), &mut s1, &rust).unwrap();

    let rust_driver = RustDriver::new(&rust);
    let toy_driver = ToyDriver::new();
    let registry = Registry::new(vec![&rust_driver, &toy_driver]);
    let r2 = index_with(tree2.path(), &mut s2, &registry).unwrap();

    assert_eq!(r1, r2);
    assert_eq!(
        s1.workspace_fingerprint().unwrap(),
        s2.workspace_fingerprint().unwrap()
    );
    assert_eq!(s1.source_skipped().unwrap(), s2.source_skipped().unwrap());
    assert_eq!(s1.canonical_dump().unwrap(), s2.canonical_dump().unwrap());
}

#[test]
fn toy_stub_end_to_end_walk_multifile_parse_error_and_diff() {
    let root = TempDir::new("toy-e2e");
    let home = TempDir::new("toy-home");
    fs::create_dir_all(root.path().join("pkg")).unwrap();
    fs::create_dir_all(root.path().join("pkg/testdata")).unwrap();
    fs::create_dir_all(root.path().join("toy_vendor")).unwrap();
    fs::write(root.path().join("toy.mod"), "example.com/demo\n").unwrap();
    fs::write(
        root.path().join("pkg/a.toy"),
        "fn Caller\ncall Caller -> Helper\nvalue_ref Caller -> Hook\n",
    )
    .unwrap();
    fs::write(root.path().join("pkg/b.toy"), "fn Helper\nfn Hook\n").unwrap();
    fs::write(root.path().join("pkg/testdata/ignored.toy"), "fn Ignored\n").unwrap();
    fs::write(root.path().join("toy_vendor/v.toy"), "fn Vendored\n").unwrap();

    git(root.path(), home.path(), &["init", "-q", "-b", "main"]);
    git(root.path(), home.path(), &["add", "."]);
    git(root.path(), home.path(), &["commit", "-q", "-m", "init"]);

    let db = TempDir::new("toy-db").path().join("graph.db");
    let mut store = Store::open(&db).unwrap();
    let rust_driver = RustDriver::owned().unwrap();
    let toy_driver = ToyDriver::new();
    let registry = Registry::new(vec![&rust_driver, &toy_driver]);

    // Walk prunes testdata and toy_vendor.
    let walked = walk_with(root.path(), &registry).unwrap();
    let paths: Vec<&str> = walked.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"pkg/a.toy"), "{paths:?}");
    assert!(paths.contains(&"pkg/b.toy"), "{paths:?}");
    assert!(
        !paths
            .iter()
            .any(|p| p.contains("testdata") || p.contains("toy_vendor"))
    );

    // Index writes non-NULL multi-file module_id and resolves cross-file call + value_ref.
    let report = index_with(root.path(), &mut store, &registry).unwrap();
    assert_eq!(report.written.len(), 2);
    assert_eq!(report.graph.unwrap().orphans, 0);
    assert_eq!(store.source_skipped().unwrap(), Some(0));

    let resolve = store.resolve_input().unwrap();
    assert!(resolve.symbols.iter().all(|s| s.module.is_some()));
    let helper = store.symbols_matching("Helper").unwrap();
    assert_eq!(helper.len(), 1);
    assert_eq!(store.callers(helper[0].id).unwrap().len(), 1);
    assert_eq!(store.ref_ids_of_kind("value_ref").unwrap().len(), 1);

    // stored_graph_with agrees (orphans == 0).
    let stored = stored_graph_with(&store, root.path(), &registry).unwrap();
    assert_eq!(stored.orphans, 0);

    // probe_with is clean right after index (setting index_head like CLI index).
    let head = phantom_fractal_git::head(root.path()).unwrap();
    store.set_index_head(head.as_deref()).unwrap();
    let fresh = probe_with(root.path(), &store, &registry).unwrap();
    assert!(!fresh.stale(), "{fresh:?}");

    // Editing toy.mod triggers probe fingerprint_changed and diff manifest_changed.
    fs::write(root.path().join("toy.mod"), "example.com/demo/v2\n").unwrap();
    let after_mod = probe_with(root.path(), &store, &registry).unwrap();
    assert!(after_mod.fingerprint_changed);
    assert!(after_mod.stale());
    let d_mod = diff_with(root.path(), &Base::Head, &registry)
        .unwrap()
        .unwrap();
    assert!(d_mod.manifest_changed);
    assert!(d_mod.changed.is_empty());
    assert!(d_mod.files_only.iter().any(|f| f.path == "toy.mod"));

    // Restore toy.mod, introduce syntax error in pkg/b.toy -> source_skipped = 1 -> exact: false.
    fs::write(root.path().join("toy.mod"), "example.com/demo\n").unwrap();
    fs::write(
        root.path().join("pkg/b.toy"),
        "// SYNTAX_ERROR
fn Helper\nfn Hook\n",
    )
    .unwrap();
    let d_body = diff_with(root.path(), &Base::Head, &registry)
        .unwrap()
        .unwrap();
    assert!(
        d_body
            .changed
            .iter()
            .any(|c| c.path == "pkg/b.toy" && c.change == FileChange::Modified)
            || d_body.files_only.iter().any(|f| f.path == "pkg/b.toy"),
        "{d_body:?}"
    );

    let err_report = index_with(root.path(), &mut store, &registry).unwrap();
    assert!(err_report.graph.is_some());
    assert_eq!(store.source_skipped().unwrap(), Some(1));
    assert_eq!(store.rs_skipped().unwrap(), Some(1));

    // Clean pass without touching files keeps source_skipped = 1.
    let noop = index_with(root.path(), &mut store, &registry).unwrap();
    assert!(noop.graph.is_none());
    assert_eq!(store.source_skipped().unwrap(), Some(1));

    // Mixed Rust + multi-package Toy repo + orphans (Rust & Toy):
    fs::create_dir_all(root.path().join("pkg2")).unwrap();
    fs::write(
        root.path().join("pkg2/c.toy"),
        "fn Helper2\ncall Helper2 -> MissingToy\n",
    )
    .unwrap();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"mix\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\next = \"1.0\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub fn rust_fn() { rust_callee(); ext::f(); }\npub fn rust_callee() {}\n",
    )
    .unwrap();
    fs::write(
        root.path().join("src/orphan.rs"),
        "pub fn orphaned_rs() {}\n",
    )
    .unwrap();
    let mixed = index_with(root.path(), &mut store, &registry).unwrap();
    let mg = mixed.graph.unwrap();
    assert_eq!(mg.counts.edges, 3);
    assert_eq!(mg.counts.unresolved, 1);
    assert_eq!(mg.counts.external, 1);
    let stored_mix = stored_graph_with(&store, root.path(), &registry).unwrap();
    assert_eq!(stored_mix.counts, mg.counts);
    assert_eq!(stored_mix.edges_by_reason, mg.edges_by_reason);
    assert_eq!(stored_mix.orphans, 1);
    assert_eq!(store.symbols_matching("rust_fn").unwrap().len(), 1);
    assert_eq!(store.symbols_matching("Helper").unwrap().len(), 1);
    assert_eq!(store.symbols_matching("Helper2").unwrap().len(), 1);

    // Manifest-only Toy pass (0 .toy files) still builds Toy package row.
    let man_dir = TempDir::new("toy-manifest-only");
    fs::write(man_dir.path().join("toy.mod"), "example.com/onlymod\n").unwrap();
    let man_db = TempDir::new("toy-man-db").path().join("g.db");
    let mut man_store = Store::open(&man_db).unwrap();
    let man_rep = index_with(man_dir.path(), &mut man_store, &registry).unwrap();
    assert!(man_rep.graph.is_some());
    assert!(
        man_store
            .canonical_dump()
            .unwrap()
            .contains("example.com/onlymod")
    );

    // Source-only Toy pass (no toy.mod) still extracts & indexes files.
    let no_mod = TempDir::new("toy-no-mod");
    fs::write(no_mod.path().join("orphan.toy"), "fn Solo\n").unwrap();
    let nm_db = TempDir::new("toy-nm-db").path().join("g.db");
    let mut nm_store = Store::open(&nm_db).unwrap();
    let nm_rep = index_with(no_mod.path(), &mut nm_store, &registry).unwrap();
    assert!(nm_rep.graph.is_some());
    assert_eq!(nm_store.symbols_matching("Solo").unwrap().len(), 1);
}
