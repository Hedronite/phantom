//! The crate-graph and edge rebuild of one index pass (SPEC-w3 G2c), and
//! the same counts read back when a pass rebuilds nothing.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use phantom_fractal_lang::{Extract, Id, Link, ResolveCounts, ResolveInput};
use phantom_fractal_lang_rust::links;
use phantom_fractal_store::{Store, StoreError};

use crate::deps;
use crate::fingerprint::{Contents, ManifestRead, Stamp, read_manifests};
use crate::graph::Manifest;
use crate::registry::{
    LanguageDriver, ManifestFile, PackageGraph, Registry, SourceFileExtract, is_named_file,
    scope_input,
};
use crate::walk::{Candidate, walk};
use crate::{EXTRACTOR_VERSION, GraphReport, IndexError, ReasonCounts};

/// Builds packages, targets, and modules from the kept files, writes them,
/// resolves the stored call references, replaces the call edges, writes
/// the `import` and `impl` links, writes each method's owning type, and
/// stores the extractor version, then the workspace fingerprint and `.rs`
/// skip count.
///
/// Module declarations are not stored, so each kept `.rs` file is read and
/// extracted again here. A file that no longer reads as UTF-8 source
/// counts as having no declarations. The same extract gives each file's
/// own test flags, which the store restores before it marks the symbols
/// of test modules, so a module that stops being a test module leaves no
/// mark behind.
struct ExtractedSources {
    by_driver: Vec<Vec<SourceFileExtract>>,
    raw_sources: HashMap<String, String>,
    own_tests: Vec<(String, Vec<bool>)>,
    parse_errors: u64,
}

pub(crate) fn kept_manifest_files_pub(manifests: &[ManifestRead]) -> Vec<ManifestFile> {
    kept_manifest_files(manifests)
}

pub(crate) fn rebuild_with(
    manifests: &[ManifestRead],
    stamp: &Stamp,
    kept: &[&Candidate],
    prefilter: &crate::PrefilterState,
    store: &mut Store,
    registry: &Registry<'_>,
) -> Result<GraphReport, IndexError> {
    let manifest_files = kept_manifest_files(manifests);
    let extracted = extract_kept_files(kept, store, registry)?;
    let (graph, driver_targets) = write_merged_graph(store, registry, &manifest_files, &extracted)?;
    let rust_manifests = cargo_manifests(&manifest_files);
    let input = resolve_input(store, &rust_manifests)?;
    let resolved = resolve_drivers(registry, &driver_targets, &input);
    persist_resolved(
        store,
        &resolved,
        prefilter,
        stamp.fingerprint,
        stamp.source_skipped + extracted.parse_errors,
    )?;
    Ok(GraphReport {
        modules: graph.modules.len(),
        orphans: graph.orphans,
        counts: resolved.counts,
        edges_by_reason: ReasonCounts::of(&resolved.calls),
    })
}

fn write_merged_graph(
    store: &mut Store,
    registry: &Registry<'_>,
    manifest_files: &[ManifestFile],
    extracted: &ExtractedSources,
) -> Result<(PackageGraph, Vec<HashSet<Id>>), IndexError> {
    for (path, flags) in &extracted.own_tests {
        store.reset_symbol_test_flags(path, flags)?;
    }
    let (graph, driver_targets) = merge_driver_graphs(
        registry,
        manifest_files,
        &extracted.by_driver,
        &extracted.raw_sources,
    );
    store.write_crate_graph_with_files(
        &graph.packages,
        &graph.targets,
        &graph.modules,
        &graph.file_modules,
        &graph.test_modules,
    )?;
    Ok((graph, driver_targets))
}

struct ResolvedGraph {
    calls: Vec<phantom_fractal_lang::Edge>,
    links: Vec<Link>,
    counts: ResolveCounts,
    owners: Vec<(Id, Id)>,
}

fn persist_resolved(
    store: &mut Store,
    resolved: &ResolvedGraph,
    prefilter: &crate::PrefilterState,
    fingerprint: [u8; 32],
    source_skipped: u64,
) -> Result<(), IndexError> {
    persist_graph_rows(store, resolved)?;
    persist_build_excluded(store, prefilter)?;
    store.set_extractor_version(EXTRACTOR_VERSION)?;
    store.set_workspace_fingerprint(&fingerprint, source_skipped)?;
    Ok(())
}

fn persist_graph_rows(store: &mut Store, resolved: &ResolvedGraph) -> Result<(), IndexError> {
    store.replace_edges(&resolved.calls)?;
    store.write_links(&resolved.links)?;
    store.write_method_owners(&resolved.owners)?;
    Ok(())
}

fn persist_build_excluded(
    store: &mut Store,
    prefilter: &crate::PrefilterState,
) -> Result<(), IndexError> {
    if should_persist_excluded(store, prefilter)? {
        store.set_build_excluded_count(prefilter.build_excluded_count)?;
        store.set_build_excluded_terms(&prefilter.build_excluded_terms)?;
    }
    Ok(())
}

fn should_persist_excluded(
    store: &Store,
    prefilter: &crate::PrefilterState,
) -> Result<bool, IndexError> {
    if prefilter.has_filtered_driver || prefilter.build_excluded_count > 0 {
        return Ok(true);
    }
    Ok(store.build_excluded_count()?.is_some())
}

fn extract_kept_files(
    kept: &[&Candidate],
    store: &Store,
    registry: &Registry<'_>,
) -> Result<ExtractedSources, IndexError> {
    let drivers = registry.drivers();
    let mut out = ExtractedSources {
        by_driver: vec![Vec::new(); drivers.len()],
        raw_sources: HashMap::with_capacity(kept.len()),
        own_tests: Vec::with_capacity(kept.len()),
        parse_errors: 0,
    };
    for file in kept {
        extract_one_kept(file, store, &drivers, &mut out)?;
    }
    Ok(out)
}

fn extract_one_kept(
    file: &Candidate,
    store: &Store,
    drivers: &[&dyn LanguageDriver],
    out: &mut ExtractedSources,
) -> Result<(), IndexError> {
    let Some(idx) = driver_index_for(drivers, &file.path) else {
        return Ok(());
    };
    let lang = drivers[idx].language();
    let extract = match Contents::read(file)? {
        Contents::Text(text) => {
            let parsed = parse_kept_text(&file.path, &text, lang, store, out)?;
            out.raw_sources.insert(file.path.clone(), text);
            parsed
        }
        Contents::Unread(_) => Extract::default(),
    };
    out.by_driver[idx].push(SourceFileExtract {
        path: file.path.clone(),
        extract,
    });
    Ok(())
}

fn parse_kept_text(
    path: &str,
    text: &str,
    lang: &dyn phantom_fractal_lang::Language,
    store: &Store,
    out: &mut ExtractedSources,
) -> Result<Extract, IndexError> {
    if lang.has_parse_errors(text) {
        out.parse_errors += 1;
    }
    let extract = lang.extract(text).map_err(StoreError::Extract)?;
    if is_stored_text(store, path, text)? {
        out.own_tests
            .push((path.to_owned(), own_test_flags(&extract)));
    }
    Ok(extract)
}

fn driver_index_for(drivers: &[&dyn LanguageDriver], path: &str) -> Option<usize> {
    let ext = Path::new(path).extension().and_then(|ext| ext.to_str())?;
    drivers
        .iter()
        .position(|driver| driver.language().extensions().contains(&ext))
}

fn driver_is_active(
    driver: &dyn LanguageDriver,
    manifests: &[ManifestFile],
    files: &[SourceFileExtract],
) -> bool {
    !files.is_empty()
        || manifests.iter().any(|m| {
            Path::new(&m.path)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| driver.manifest_names().contains(&n))
        })
}

fn merge_driver_graphs(
    registry: &Registry<'_>,
    manifests: &[ManifestFile],
    by_driver: &[Vec<SourceFileExtract>],
    raw_sources: &HashMap<String, String>,
) -> (PackageGraph, Vec<HashSet<Id>>) {
    let drivers = registry.drivers();
    let active: Vec<usize> = drivers
        .iter()
        .enumerate()
        .filter(|(idx, d)| driver_is_active(**d, manifests, &by_driver[*idx]))
        .map(|(idx, _)| idx)
        .collect();
    if active.len() == 1 {
        let idx = active[0];
        let single = drivers[idx].build_graph_with_sources(manifests, &by_driver[idx], raw_sources);
        let mut target_sets = vec![HashSet::new(); drivers.len()];
        target_sets[idx] = single.targets.iter().map(|t| t.id).collect();
        return (single, target_sets);
    }
    if drivers.len() == 1 {
        let single = drivers[0].build_graph_with_sources(manifests, &by_driver[0], raw_sources);
        let ids: HashSet<Id> = single.targets.iter().map(|t| t.id).collect();
        return (single, vec![ids]);
    }
    let mut merged = PackageGraph::default();
    let mut target_sets = Vec::with_capacity(drivers.len());
    let mut next_target: Id = 1;
    let mut next_module: Id = 1;
    for (idx, driver) in drivers.iter().enumerate() {
        if !active.contains(&idx) {
            target_sets.push(HashSet::new());
            continue;
        }
        let part = driver.build_graph_with_sources(manifests, &by_driver[idx], raw_sources);
        let ids = append_package_graph(&mut merged, part, &mut next_target, &mut next_module);
        target_sets.push(ids);
    }
    (merged, target_sets)
}

fn append_package_graph(
    merged: &mut PackageGraph,
    part: PackageGraph,
    next_target: &mut Id,
    next_module: &mut Id,
) -> HashSet<Id> {
    merged.packages.extend(part.packages);
    merged.orphans += part.orphans;
    let (target_map, own_targets) = remap_targets(part.targets, next_target, &mut merged.targets);
    let mod_map = remap_modules(part.modules, &target_map, next_module, &mut merged.modules);
    remap_module_refs(part.file_modules, part.test_modules, &mod_map, merged);
    own_targets
}

fn remap_targets(
    targets: Vec<phantom_fractal_lang::Target>,
    next_target: &mut Id,
    out: &mut Vec<phantom_fractal_lang::Target>,
) -> (HashMap<Id, Id>, HashSet<Id>) {
    let mut target_map = HashMap::new();
    let mut own_targets = HashSet::new();
    for mut t in targets {
        let new_id = *next_target;
        *next_target += 1;
        target_map.insert(t.id, new_id);
        t.id = new_id;
        own_targets.insert(new_id);
        out.push(t);
    }
    (target_map, own_targets)
}

fn remap_modules(
    modules: Vec<phantom_fractal_lang::Module>,
    target_map: &HashMap<Id, Id>,
    next_module: &mut Id,
    out: &mut Vec<phantom_fractal_lang::Module>,
) -> HashMap<Id, Id> {
    let mut mod_map = HashMap::new();
    for m in &modules {
        let new_id = *next_module;
        *next_module += 1;
        mod_map.insert(m.id, new_id);
    }
    for mut m in modules {
        m.id = mod_map[&m.id];
        m.target = target_map[&m.target];
        m.parent = m.parent.and_then(|p| mod_map.get(&p).copied());
        out.push(m);
    }
    mod_map
}

fn remap_module_refs(
    file_modules: Vec<phantom_fractal_lang::FileModule>,
    test_modules: Vec<Id>,
    mod_map: &HashMap<Id, Id>,
    merged: &mut PackageGraph,
) {
    for mut fm in file_modules {
        fm.module = mod_map[&fm.module];
        merged.file_modules.push(fm);
    }
    for tm in test_modules {
        if let Some(&new_id) = mod_map.get(&tm) {
            merged.test_modules.push(new_id);
        }
    }
}

fn resolve_drivers(
    registry: &Registry<'_>,
    driver_targets: &[HashSet<Id>],
    input: &ResolveInput,
) -> ResolvedGraph {
    let drivers = registry.drivers();
    let active: Vec<usize> = drivers
        .iter()
        .enumerate()
        .filter(|(idx, d)| {
            !driver_targets[*idx].is_empty()
                || input
                    .symbols
                    .iter()
                    .any(|s| has_driver_ext(&s.file, d.language().extensions()))
        })
        .map(|(idx, _)| idx)
        .collect();
    if active.len() == 1 || drivers.len() == 1 {
        let idx = active.first().copied().unwrap_or(0);
        let g = drivers[idx].resolver().resolve_graph(input);
        return ResolvedGraph {
            calls: g.calls,
            links: g.links,
            counts: g.counts,
            owners: drivers[idx].method_owners(input),
        };
    }
    let mut out = ResolvedGraph {
        calls: Vec::new(),
        links: Vec::new(),
        counts: ResolveCounts::default(),
        owners: Vec::new(),
    };
    for (idx, driver) in drivers.iter().enumerate() {
        if !active.contains(&idx) {
            continue;
        }
        let scoped = scope_input(input, driver.language().extensions(), &driver_targets[idx]);
        let g = driver.resolver().resolve_graph(&scoped);
        out.calls.extend(g.calls);
        out.links.extend(g.links);
        add_counts(&mut out.counts, g.counts);
        out.owners.extend(driver.method_owners(&scoped));
    }
    out
}

fn has_driver_ext(path: &str, exts: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.contains(&e))
}

fn add_counts(total: &mut ResolveCounts, part: ResolveCounts) {
    total.edges += part.edges;
    total.unresolved += part.unresolved;
    total.external += part.external;
    total.no_source += part.no_source;
    total.method_calls += part.method_calls;
    total.macros += part.macros;
}

/// The graph report of the tree at `root` as last indexed into `store`,
/// read without writing: for an index pass that rebuilt nothing.
pub fn stored_graph(store: &Store, root: &Path) -> Result<GraphReport, IndexError> {
    let registry = crate::default_registry().map_err(StoreError::Extract)?;
    stored_graph_with(store, root, &registry)
}

/// Like [`stored_graph`], over any registered language set.
pub fn stored_graph_with(
    store: &Store,
    root: &Path,
    registry: &Registry<'_>,
) -> Result<GraphReport, IndexError> {
    let walk = crate::walk::walk_with(root, registry)?;
    let manifest_reads = crate::fingerprint::read_manifests_with(&walk, registry)?;
    let manifests = cargo_manifests(&kept_manifest_files(&manifest_reads));
    let input = resolve_input(store, &manifests)?;
    let driver_targets: Vec<HashSet<Id>> = registry
        .drivers()
        .iter()
        .map(|d| targets_for_driver(&input, d.language().extensions()))
        .collect();
    let resolved = resolve_drivers(registry, &driver_targets, &input);
    let orphans = count_orphans(store, &input)?;
    Ok(GraphReport {
        modules: input.modules.len(),
        orphans,
        counts: resolved.counts,
        edges_by_reason: ReasonCounts::of(&resolved.calls),
    })
}

fn targets_for_driver(input: &ResolveInput, exts: &[&str]) -> HashSet<Id> {
    input
        .symbols
        .iter()
        .filter(|s| {
            Path::new(&s.file)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .filter_map(|s| s.module)
        .filter_map(|mid| input.modules.iter().find(|m| m.id == mid).map(|m| m.target))
        .collect()
}

fn count_orphans(store: &Store, _input: &ResolveInput) -> Result<usize, IndexError> {
    let mut orphans = 0;
    for file in store.files()? {
        if !store.file_has_module(file.id)? {
            orphans += 1;
        }
    }
    Ok(orphans)
}

/// The `(method, type)` owner pairs of the tree at `root` as last indexed
/// into `store`, read without writing: the pairs [`rebuild`] passes to
/// `write_method_owners`, from the same resolver input.
pub fn stored_method_owners(store: &Store, root: &Path) -> Result<Vec<(Id, Id)>, IndexError> {
    let registry = crate::default_registry().map_err(StoreError::Extract)?;
    let walk = crate::walk::walk_with(root, &registry)?;
    let manifests = kept_manifests(&crate::fingerprint::read_manifests_with(&walk, &registry)?);
    let input = resolve_input(store, &manifests)?;
    let driver_targets: Vec<HashSet<Id>> = registry
        .drivers()
        .iter()
        .map(|d| targets_for_driver(&input, d.language().extensions()))
        .collect();
    Ok(resolve_drivers(&registry, &driver_targets, &input).owners)
}

/// The resolver input of the tree at `root` as last indexed into `store`,
/// read without writing: the store's rows and the `deps` its manifests
/// allow, as [`rebuild`] reads them.
pub fn stored_input(store: &Store, root: &Path) -> Result<ResolveInput, IndexError> {
    let manifests = kept_manifests(&read_manifests(&walk(root)?)?);
    resolve_input(store, &manifests)
}

/// The `import` and `impl` links of the tree at `root` as last indexed
/// into `store`, read without writing: the links [`rebuild`] passes to
/// `write_links`, from the same resolver input (SPEC-w7 G0a).
pub fn stored_links(store: &Store, root: &Path) -> Result<Vec<Link>, IndexError> {
    let manifests = kept_manifests(&read_manifests(&walk(root)?)?);
    Ok(links(&resolve_input(store, &manifests)?))
}

/// A file's own test flags, one per symbol in the order the store
/// inserts them: items, then methods.
fn own_test_flags(extract: &Extract) -> Vec<bool> {
    extract
        .items
        .iter()
        .map(|item| item.is_test)
        .chain(extract.methods.iter().map(|method| method.is_test))
        .collect()
}

/// True when the store holds `path` with the blake3 of `text`.
fn is_stored_text(store: &Store, path: &str, text: &str) -> Result<bool, IndexError> {
    let hash = blake3::hash(text.as_bytes());
    Ok(store
        .stored_blake3(path)?
        .is_some_and(|stored| stored.as_slice() == hash.as_bytes()))
}

/// What the resolver reads: the store's rows, and the `deps` the walked
/// manifests allow. Both [`rebuild`] and [`stored_graph`] build it here,
/// from the same walk of the root, so they agree.
fn resolve_input(store: &Store, manifests: &[Manifest]) -> Result<ResolveInput, IndexError> {
    let mut input = store.resolve_input()?;
    input.deps = deps::scan(manifests);
    Ok(input)
}

/// Kept manifest files as UTF-8 text, in walk order.
fn kept_manifest_files(manifests: &[ManifestRead]) -> Vec<ManifestFile> {
    manifests
        .iter()
        .filter_map(|manifest| match &manifest.contents {
            Contents::Text(text) => Some(ManifestFile {
                path: manifest.path.clone(),
                text: text.clone(),
            }),
            Contents::Unread(_) => None,
        })
        .collect()
}

fn cargo_manifests(manifests: &[ManifestFile]) -> Vec<Manifest> {
    manifests
        .iter()
        .filter(|manifest| is_named_file(&manifest.path, "Cargo.toml"))
        .map(|manifest| Manifest {
            path: manifest.path.clone(),
            text: manifest.text.clone(),
        })
        .collect()
}

fn kept_manifests(manifests: &[ManifestRead]) -> Vec<Manifest> {
    cargo_manifests(&kept_manifest_files(manifests))
}

#[cfg(test)]
mod tests {
    use super::*;
    use phantom_fractal_lang::resolve::{Module, Target, TargetKind};

    #[test]
    fn merge_helpers_accumulate_orphans_ids_and_counts() {
        let mut total = ResolveCounts {
            edges: 2,
            unresolved: 2,
            external: 2,
            no_source: 2,
            method_calls: 2,
            macros: 2,
        };
        let part = ResolveCounts {
            edges: 3,
            unresolved: 3,
            external: 3,
            no_source: 3,
            method_calls: 3,
            macros: 3,
        };
        add_counts(&mut total, part);
        assert_eq!(total.edges, 5);
        assert_eq!(total.unresolved, 5);
        assert_eq!(total.external, 5);
        assert_eq!(total.no_source, 5);
        assert_eq!(total.method_calls, 5);
        assert_eq!(total.macros, 5);

        let mut merged = PackageGraph {
            orphans: 2,
            ..PackageGraph::default()
        };
        let mut next_target = 1;
        let mut next_module = 1;
        let g = PackageGraph {
            orphans: 3,
            targets: vec![Target {
                id: 1,
                package: "p".to_owned(),
                name: "p".to_owned(),
                kind: TargetKind::Lib,
                root: "src/lib.rs".to_owned(),
            }],
            modules: vec![
                Module {
                    id: 1,
                    target: 1,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "src/lib.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 2,
                    target: 1,
                    parent: Some(1),
                    path: "crate::sub".to_owned(),
                    file: "src/sub.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            ..PackageGraph::default()
        };
        let _ = append_package_graph(&mut merged, g, &mut next_target, &mut next_module);
        assert_eq!(merged.orphans, 5);
        assert_eq!(next_module, 3);
        assert_eq!(merged.modules.len(), 2);
        assert_eq!(merged.modules[1].id, 2);
    }
}
