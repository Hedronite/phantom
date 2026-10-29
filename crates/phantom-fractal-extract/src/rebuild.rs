//! The crate-graph and edge rebuild of one index pass (SPEC-w3 G2c), and
//! the same counts read back when a pass rebuilds nothing.

use std::collections::HashSet;
use std::path::Path;

use phantom_fractal_lang::{Id, Language, Link, ResolveInput};
use phantom_fractal_lang_rust::{RustLanguage, links, method_owners, resolve, resolve_graph};
use phantom_fractal_store::{Store, StoreError};

use crate::deps;
use crate::fingerprint::{Contents, ManifestRead, Stamp, read_manifests};
use crate::graph::{self, Manifest, SourceFile};
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
/// counts as having no declarations.
pub(crate) fn rebuild(
    manifests: &[ManifestRead],
    stamp: &Stamp,
    kept: &[&Candidate],
    store: &mut Store,
    language: &RustLanguage,
) -> Result<GraphReport, IndexError> {
    let manifests = kept_manifests(manifests);
    let mut files = Vec::with_capacity(kept.len());
    for file in kept {
        let mod_decls = match Contents::read(file)? {
            Contents::Text(text) => {
                language
                    .extract(&text)
                    .map_err(StoreError::Extract)?
                    .mod_decls
            }
            Contents::Unread(_) => Vec::new(),
        };
        files.push(SourceFile {
            path: file.path.clone(),
            mod_decls,
        });
    }
    let graph = graph::build(&manifests, &files);
    store.write_crate_graph(
        &graph.packages,
        &graph.targets,
        &graph.modules,
        &graph.test_modules,
    )?;
    let input = resolve_input(store, &manifests)?;
    let resolved = resolve_graph(&input);
    store.replace_edges(&resolved.calls)?;
    store.write_links(&resolved.links)?;
    store.write_method_owners(&method_owners(&input))?;
    store.set_extractor_version(EXTRACTOR_VERSION)?;
    store.set_workspace_fingerprint(&stamp.fingerprint, stamp.rs_skipped)?;
    Ok(GraphReport {
        modules: graph.modules.len(),
        orphans: graph.orphans.len(),
        counts: resolved.counts,
        edges_by_reason: ReasonCounts::of(&resolved.calls),
    })
}

/// The graph report of the tree at `root` as last indexed into `store`,
/// read without writing: for an index pass that rebuilt nothing. The
/// resolver is pure and the input is built as the index pass builds it, so
/// its counts are the ones the last rebuild found. An orphan is a stored
/// file no module reaches.
pub fn stored_graph(store: &Store, root: &Path) -> Result<GraphReport, IndexError> {
    let manifests = kept_manifests(&read_manifests(&walk(root)?)?);
    let input = resolve_input(store, &manifests)?;
    let (edges, counts) = resolve(&input);
    let reached: HashSet<&str> = input
        .modules
        .iter()
        .map(|module| module.file.as_str())
        .collect();
    let orphans = store
        .file_paths()?
        .iter()
        .filter(|path| !reached.contains(path.as_str()))
        .count();
    Ok(GraphReport {
        modules: input.modules.len(),
        orphans,
        counts,
        edges_by_reason: ReasonCounts::of(&edges),
    })
}

/// The `(method, type)` owner pairs of the tree at `root` as last indexed
/// into `store`, read without writing: the pairs [`rebuild`] passes to
/// `write_method_owners`, from the same resolver input.
pub fn stored_method_owners(store: &Store, root: &Path) -> Result<Vec<(Id, Id)>, IndexError> {
    let manifests = kept_manifests(&read_manifests(&walk(root)?)?);
    Ok(method_owners(&resolve_input(store, &manifests)?))
}

/// The `import` and `impl` links of the tree at `root` as last indexed
/// into `store`, read without writing: the links [`rebuild`] passes to
/// `write_links`, from the same resolver input (SPEC-w7 G0a).
pub fn stored_links(store: &Store, root: &Path) -> Result<Vec<Link>, IndexError> {
    let manifests = kept_manifests(&read_manifests(&walk(root)?)?);
    Ok(links(&resolve_input(store, &manifests)?))
}

/// What the resolver reads: the store's rows, and the `deps` the walked
/// manifests allow. Both [`rebuild`] and [`stored_graph`] build it here,
/// from the same walk of the root, so they agree.
fn resolve_input(store: &Store, manifests: &[Manifest]) -> Result<ResolveInput, IndexError> {
    let mut input = store.resolve_input()?;
    input.deps = deps::scan(manifests);
    Ok(input)
}

/// The manifests the bounded read kept as UTF-8 text, in walk order.
fn kept_manifests(manifests: &[ManifestRead]) -> Vec<Manifest> {
    manifests
        .iter()
        .filter_map(|manifest| match &manifest.contents {
            Contents::Text(text) => Some(Manifest {
                path: manifest.path.clone(),
                text: text.clone(),
            }),
            Contents::Unread(_) => None,
        })
        .collect()
}
