//! The crate-graph and edge rebuild of one index pass (SPEC-w3 G2c).

use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::{RustLanguage, resolve};
use phantom_fractal_store::{Store, StoreError};

use crate::graph::{self, Manifest, SourceFile};
use crate::read::{Source, read_source};
use crate::walk::{Candidate, Walk};
use crate::{EXTRACTOR_VERSION, GraphReport, IndexError};

/// Builds packages, targets, and modules from the kept files, writes them,
/// resolves the stored call references, replaces the edges, and stores the
/// extractor version.
///
/// Module declarations are not stored, so each kept `.rs` file is read and
/// extracted again here. A file that no longer reads as UTF-8 source
/// counts as having no declarations.
pub(crate) fn rebuild(
    walk: &Walk,
    kept: &[&Candidate],
    store: &mut Store,
    language: &RustLanguage,
) -> Result<GraphReport, IndexError> {
    let manifests: Vec<Manifest> = walk
        .files
        .iter()
        .filter(|file| file.path == "Cargo.toml" || file.path.ends_with("/Cargo.toml"))
        .filter_map(|file| {
            read_text(file).transpose().map(|text| {
                text.map(|text| Manifest {
                    path: file.path.clone(),
                    text,
                })
            })
        })
        .collect::<Result<_, _>>()?;
    let mut files = Vec::with_capacity(kept.len());
    for file in kept {
        let mod_decls = match read_text(file)? {
            Some(text) => {
                language
                    .extract(&text)
                    .map_err(StoreError::Extract)?
                    .mod_decls
            }
            None => Vec::new(),
        };
        files.push(SourceFile {
            path: file.path.clone(),
            mod_decls,
        });
    }
    let graph = graph::build(&manifests, &files);
    store.write_crate_graph(&graph.packages, &graph.targets, &graph.modules)?;
    let input = store.resolve_input()?;
    let (edges, counts) = resolve(&input);
    store.replace_edges(&edges)?;
    store.set_extractor_version(EXTRACTOR_VERSION)?;
    Ok(GraphReport {
        modules: graph.modules.len(),
        orphans: graph.orphans.len(),
        counts,
    })
}

/// The file's text through the bounded read, or none if it is skipped or
/// not UTF-8.
fn read_text(file: &Candidate) -> Result<Option<String>, IndexError> {
    Ok(match read_source(&file.abs_path)? {
        Source::Bytes { bytes, .. } => String::from_utf8(bytes).ok(),
        Source::Skipped(_) => None,
    })
}
