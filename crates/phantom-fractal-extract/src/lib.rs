//! Walk a Rust tree, hash each file, and persist its extract (canon §6.4
//! steps 2, 3, and 5).
//!
//! The walk is language-agnostic; [`index`] keeps the `.rs` files and writes
//! each through [`Store::put_source`], then rebuilds the crate graph and the
//! call edges when anything changed (SPEC-w3 G2c). The walked tree is only
//! read; nothing in it is compiled or run.
#![forbid(unsafe_code)]

pub mod deps;
mod diff;
mod fingerprint;
mod fresh;
pub mod graph;
mod read;
mod rebuild;
pub mod registry;
mod walk;

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use phantom_fractal_git::GitError;
use phantom_fractal_lang::{Edge, Reason, ResolveCounts};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{IndexState, Store, StoreError, SymbolMatch, parse_path_line};

use fingerprint::{Unread, read_manifests_with, stamp};

pub use diff::{ChangedSymbol, Diff, DiffError, FileOnly, diff, diff_with, symbol_keys};
pub use fresh::{Freshness, probe, probe_with};
pub use phantom_fractal_lang as lang;
pub use read::{Source, read_source};
pub use rebuild::{
    stored_graph, stored_graph_with, stored_input, stored_links, stored_method_owners,
};
pub use registry::{
    LanguageDriver, ManifestFile, PackageGraph, Registry, RustDriver, RustResolver,
    SourceFileExtract, default_registry,
};
pub use walk::{Candidate, MAX_FILE_BYTES, SkipReason, Skipped, Walk, is_secret, walk, walk_with};

/// Version of the extraction this walker writes, stored after each graph
/// rebuild. Bump it when the stored rows for an unchanged file would differ:
/// a database stamped with any other version is fully re-extracted. Version
/// 2 adds call references, methods, scopes, and edges; version 3 adds the
/// `self-method` and `self-assoc` edges; version 4 adds `crate-path` edges;
/// version 5 adds `macro_arg` refs and test marking; version 6 adds the
/// end line, signature, and doc summary of items and methods; version 7
/// adds the `import` and `impl` links; version 8 adds receiver type hints
/// and the `typed-method` edges; version 9 adds `test` targets and their
/// module trees; version 10 places `macro_arg` refs, with their receivers,
/// hints and shadowing; version 11 adds the return, path-call and
/// same-file field hints; version 12 adds the `use-glob` edges and the
/// parse budget; version 13 stores the visibility of each `use` row, so a
/// `pub use` re-export reads differently from a private import.
pub const EXTRACTOR_VERSION: u32 = 15;

/// What one [`index`] pass did, by repo-relative path, in walk order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// Extracted and written this pass.
    pub written: Vec<String>,
    /// Already stored with the same hash; nothing written.
    pub unchanged: Vec<String>,
    /// `.rs` files the walk kept whose bytes are not UTF-8.
    pub not_utf8: Vec<String>,
    /// Everything the walk skipped, any extension.
    pub skipped: Vec<Skipped>,
    /// Stored `.rs` paths this pass no longer keeps; their rows are gone.
    pub removed: Vec<String>,
    /// The crate graph and resolve pass, or none when nothing changed and
    /// the stored extractor version is current, so nothing was rebuilt.
    pub graph: Option<GraphReport>,
}

/// What one crate-graph rebuild found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GraphReport {
    pub modules: usize,
    /// Kept `.rs` files no target reaches.
    pub orphans: usize,
    pub counts: ResolveCounts,
    /// The edges placed, by reason.
    pub edges_by_reason: ReasonCounts,
}

/// Edges placed for each of the nine reasons, zero included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReasonCounts {
    pub path_exact: u64,
    pub local: u64,
    pub use_alias: u64,
    pub assoc: u64,
    pub self_method: u64,
    pub self_assoc: u64,
    pub crate_path: u64,
    pub typed_method: u64,
    pub unique_method: u64,
    pub use_glob: u64,
}

impl ReasonCounts {
    /// Every reason in declaration order.
    pub const REASONS: [Reason; 10] = [
        Reason::PathExact,
        Reason::Local,
        Reason::UseAlias,
        Reason::Assoc,
        Reason::SelfMethod,
        Reason::SelfAssoc,
        Reason::CratePath,
        Reason::TypedMethod,
        Reason::UniqueMethod,
        Reason::UseGlob,
    ];

    /// The count of each reason among `edges`.
    pub fn of(edges: &[Edge]) -> Self {
        let mut counts = Self::default();
        for edge in edges {
            if let Some(slot) = counts.slot(edge.reason) {
                *slot += 1;
            }
        }
        counts
    }

    /// The count for `reason`.
    pub fn get(&self, reason: Reason) -> u64 {
        let mut copy = *self;
        copy.slot(reason).copied().unwrap_or(0)
    }

    fn slot(&mut self, reason: Reason) -> Option<&mut u64> {
        match reason {
            Reason::PathExact => Some(&mut self.path_exact),
            Reason::Local => Some(&mut self.local),
            Reason::UseAlias => Some(&mut self.use_alias),
            Reason::Assoc => Some(&mut self.assoc),
            Reason::SelfMethod => Some(&mut self.self_method),
            Reason::SelfAssoc => Some(&mut self.self_assoc),
            Reason::CratePath => Some(&mut self.crate_path),
            Reason::TypedMethod => Some(&mut self.typed_method),
            Reason::UniqueMethod => Some(&mut self.unique_method),
            Reason::UseGlob => Some(&mut self.use_glob),
            Reason::PackageImport | Reason::EmbeddedMethod | Reason::InterfaceMethod => None,
        }
    }
}

/// Why an [`index`] pass stopped.
#[derive(Debug)]
#[non_exhaustive]
pub enum IndexError {
    /// The root could not be walked, or a kept file could not be opened or
    /// read.
    Io(std::io::Error),
    /// The store refused a read or a write.
    Store(StoreError),
    /// The root's repository opened but its HEAD could not be read
    /// ([`probe`] only).
    Git(GitError),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "walk failed: {err}"),
            Self::Store(err) => write!(f, "store failed: {err}"),
            Self::Git(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for IndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Store(err) => Some(err),
            Self::Git(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for IndexError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<GitError> for IndexError {
    fn from(err: GitError) -> Self {
        Self::Git(err)
    }
}

impl From<StoreError> for IndexError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// Walks `root`, stores every kept `.rs` file whose hash is not already
/// stored for its path, and removes the stored paths the walk no longer
/// keeps. When anything was written or removed, the stored extractor
/// version is not [`EXTRACTOR_VERSION`], or the stored workspace
/// fingerprint is not this pass's, it then rebuilds the crate graph,
/// resolves calls, replaces the edges, and stores the version and the
/// fingerprint. A stored version other than the current one re-extracts
/// every file first.
///
/// The fingerprint covers every walked `Cargo.toml`, and every `.rs` file
/// the walk or the read skipped or that is not UTF-8. So a manifest edit
/// alone, or a change in which `.rs` files are skipped, rebuilds.
/// Resolves `query` against `store`, probing `root` when `query` is a `path:line`
/// location so stale indexes refuse line-number resolution honestly.
pub fn resolve_query(
    root: &Path,
    store: &Store,
    query: &str,
) -> Result<Vec<SymbolMatch>, IndexError> {
    if parse_path_line(query).is_some() {
        let freshness = probe(root, store)?;
        let state = if freshness.stale() {
            IndexState::Stale
        } else {
            IndexState::Fresh
        };
        return Ok(store.symbols_matching_for(query, state)?);
    }
    Ok(store.symbols_matching(query)?)
}

pub fn index(
    root: &Path,
    store: &mut Store,
    _language: &RustLanguage,
) -> Result<IndexReport, IndexError> {
    let registry = default_registry().map_err(StoreError::Extract)?;
    index_with(root, store, &registry)
}

/// Indexes every kept file claimed by any driver in `registry`.
pub fn index_with(
    root: &Path,
    store: &mut Store,
    registry: &Registry<'_>,
) -> Result<IndexReport, IndexError> {
    let walk = walk_with(root, registry)?;
    store.atomic_batch(|store| {
        let reextract = store.extractor_version()? != Some(EXTRACTOR_VERSION);
        let mut report = IndexReport {
            skipped: walk.skipped.clone(),
            ..IndexReport::default()
        };
        let kept = index_sources(&walk, store, registry, reextract, &mut report)?;
        remove_stale_files(store, &kept, &mut report)?;
        let unread = unread_sources(&report, registry);
        let manifests = read_manifests_with(&walk, registry)?;
        let stamp = stamp(&manifests, &unread);
        let stale = store.workspace_fingerprint()? != Some(stamp.fingerprint);
        if reextract || !report.written.is_empty() || !report.removed.is_empty() || stale {
            report.graph = Some(rebuild::rebuild_with(
                &manifests, &stamp, &kept, store, registry,
            )?);
        }
        Ok(report)
    })
}

fn index_sources<'a>(
    walk: &'a Walk,
    store: &mut Store,
    registry: &Registry<'_>,
    reextract: bool,
    report: &mut IndexReport,
) -> Result<Vec<&'a Candidate>, IndexError> {
    let mut kept = Vec::new();
    for file in &walk.files {
        if let Some(driver) = registry.language_for(&file.path) {
            index_one_source(file, driver.language(), store, reextract, report, &mut kept)?;
        }
    }
    Ok(kept)
}

fn index_one_source<'a>(
    file: &'a Candidate,
    language: &dyn phantom_fractal_lang::Language,
    store: &mut Store,
    reextract: bool,
    report: &mut IndexReport,
    kept: &mut Vec<&'a Candidate>,
) -> Result<(), IndexError> {
    let Some((source, mtime_ns)) = read_utf8_source(file, report)? else {
        return Ok(());
    };
    kept.push(file);
    if !reextract && is_unchanged(store, &file.path, &source)? {
        report.unchanged.push(file.path.clone());
        return Ok(());
    }
    store.put_source(&file.path, &source, mtime_ns, &DynLang(language))?;
    report.written.push(file.path.clone());
    Ok(())
}

fn read_utf8_source(
    file: &Candidate,
    report: &mut IndexReport,
) -> Result<Option<(String, i64)>, IndexError> {
    let (bytes, mtime_ns) = match read_source(&file.abs_path)? {
        Source::Bytes { bytes, mtime_ns } => (bytes, mtime_ns),
        Source::Skipped(reason) => {
            report.skipped.push(Skipped {
                path: file.path.clone(),
                reason,
            });
            return Ok(None);
        }
    };
    let Ok(source) = String::from_utf8(bytes) else {
        report.not_utf8.push(file.path.clone());
        return Ok(None);
    };
    Ok(Some((source, mtime_ns)))
}

fn remove_stale_files(
    store: &mut Store,
    kept: &[&Candidate],
    report: &mut IndexReport,
) -> Result<(), IndexError> {
    let kept_paths: HashSet<&str> = kept.iter().map(|file| file.path.as_str()).collect();
    for path in store.file_paths()? {
        if !kept_paths.contains(path.as_str()) {
            store.remove_file(&path)?;
            report.removed.push(path);
        }
    }
    Ok(())
}

fn unread_sources<'a>(report: &'a IndexReport, registry: &Registry<'_>) -> Vec<(&'a str, Unread)> {
    report
        .skipped
        .iter()
        .filter(|skip| registry.is_supported(&skip.path))
        .map(|skip| (skip.path.as_str(), Unread::Skipped))
        .chain(
            report
                .not_utf8
                .iter()
                .map(|path| (path.as_str(), Unread::NotUtf8)),
        )
        .collect()
}

/// True when the store already holds `path` with the blake3 of `source`.
fn is_unchanged(store: &Store, path: &str, source: &str) -> Result<bool, IndexError> {
    let hash = blake3::hash(source.as_bytes());
    Ok(store
        .stored_blake3(path)?
        .is_some_and(|stored| stored.as_slice() == hash.as_bytes()))
}

struct DynLang<'a>(&'a dyn phantom_fractal_lang::Language);

impl phantom_fractal_lang::Language for DynLang<'_> {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn extensions(&self) -> &'static [&'static str] {
        self.0.extensions()
    }

    fn extract(
        &self,
        source: &str,
    ) -> Result<phantom_fractal_lang::Extract, phantom_fractal_lang::ExtractError> {
        self.0.extract(source)
    }

    fn has_parse_errors(&self, source: &str) -> bool {
        self.0.has_parse_errors(source)
    }

    fn qualifier_separator(&self) -> &'static str {
        self.0.qualifier_separator()
    }
}

#[cfg(test)]
mod tests {
    use phantom_fractal_lang::{Language, ResolveInput, Resolver};
    use phantom_fractal_lang_rust::RustLanguage;

    use super::{DynLang, RustResolver};

    #[test]
    fn dyn_lang_and_rust_resolver_forward_trait_methods() {
        let rust = RustLanguage::new().expect("rust");
        let dyn_lang = DynLang(&rust);
        assert_eq!(dyn_lang.name(), "rust");
        assert_eq!(dyn_lang.extensions(), &["rs"]);
        assert_eq!(dyn_lang.qualifier_separator(), "::");
        assert!(!dyn_lang.has_parse_errors(
            "pub fn ok() {}
"
        ));
        assert_eq!(
            dyn_lang
                .extract(
                    "pub fn a() { b(); }
pub fn b() {}
"
                )
                .unwrap()
                .items
                .len(),
            2
        );

        struct CustomLang;
        impl Language for CustomLang {
            fn name(&self) -> &'static str {
                "custom"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &["c"]
            }
            fn extract(
                &self,
                _source: &str,
            ) -> Result<phantom_fractal_lang::Extract, phantom_fractal_lang::ExtractError>
            {
                Ok(Default::default())
            }
            fn has_parse_errors(&self, source: &str) -> bool {
                source.contains("ERR")
            }
            fn qualifier_separator(&self) -> &'static str {
                "."
            }
        }
        let custom = CustomLang;
        let dyn_custom = DynLang(&custom);
        assert_eq!(dyn_custom.qualifier_separator(), ".");
        assert!(dyn_custom.has_parse_errors("ERR"));
        assert!(!dyn_custom.has_parse_errors("ok"));

        let resolver = RustResolver;
        assert_eq!(resolver.name(), "rust");
        let (edges, counts) = resolver.resolve(&ResolveInput::default());
        assert!(edges.is_empty());
        assert_eq!(counts.edges, 0);
    }
}
