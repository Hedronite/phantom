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

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use phantom_fractal_git::GitError;
use phantom_fractal_lang::{Edge, Reason, ResolveCounts};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{IndexState, Store, StoreError, SymbolMatch, parse_path_line};

use fingerprint::{Unread, read_manifests_with, stamp_with_extra};

pub use diff::{ChangedSymbol, Diff, DiffError, FileOnly, diff, diff_with, symbol_keys};
pub use fresh::{Freshness, probe, probe_with};
pub use phantom_fractal_lang as lang;
pub use read::{Source, read_source};
pub use rebuild::{
    stored_graph, stored_graph_with, stored_input, stored_links, stored_method_owners,
};
pub use registry::{
    DriverFilter, GoDriver, LanguageDriver, ManifestFile, PackageGraph, Registry, RustDriver,
    RustResolver, SourceFileExtract, default_registry,
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
        let manifests = read_manifests_with(&walk, registry)?;
        let prefilter = build_driver_filters(&walk, &manifests, registry)?;
        report
            .not_utf8
            .extend(prefilter.filter_not_utf8.iter().cloned());
        let kept = index_sources(&walk, store, registry, &prefilter, reextract, &mut report)?;
        remove_stale_files(store, &kept, &mut report)?;
        let owned_unread = driver_unread_owned(&prefilter, registry);
        let mut unread = unread_sources(&report, registry);
        unread.extend(owned_unread.iter().map(|(p, u)| (p.as_str(), *u)));
        let stamp = stamp_with_extra(&manifests, &unread, &prefilter.excluded_hashes);
        let stale = store.workspace_fingerprint()? != Some(stamp.fingerprint);
        if reextract || !report.written.is_empty() || !report.removed.is_empty() || stale {
            report.graph = Some(rebuild::rebuild_with(
                &manifests, &stamp, &kept, &prefilter, store, registry,
            )?);
        }
        Ok(report)
    })
}

pub(crate) struct PrefilterState {
    pub(crate) filters: HashMap<String, Option<HashSet<String>>>,
    pub(crate) build_excluded_count: u64,
    pub(crate) build_excluded_terms: String,
    pub(crate) excluded_hashes: Vec<(String, [u8; 32])>,
    pub(crate) filter_not_utf8: Vec<String>,
    pub(crate) prefilter_skipped: Vec<String>,
    pub(crate) has_filtered_driver: bool,
}

impl PrefilterState {
    /// True when `path` passes any source filter registered for `language_name`.
    pub(crate) fn allows(&self, language_name: &str, path: &str) -> bool {
        self.filters
            .get(language_name)
            .and_then(Option::as_ref)
            .is_none_or(|allowed| allowed.contains(path))
    }

    /// True when `path` is claimed by a driver in `registry` and passes its source filter.
    pub(crate) fn keeps_source(&self, registry: &Registry<'_>, path: &str) -> bool {
        registry
            .language_for(path)
            .is_some_and(|driver| self.allows(driver.language().name(), path))
    }
}

pub(crate) fn build_driver_filters(
    walk: &Walk,
    manifests: &[fingerprint::ManifestRead],
    registry: &Registry<'_>,
) -> Result<PrefilterState, IndexError> {
    let mut state = PrefilterState {
        filters: HashMap::new(),
        build_excluded_count: 0,
        build_excluded_terms: String::new(),
        excluded_hashes: Vec::new(),
        filter_not_utf8: Vec::new(),
        prefilter_skipped: Vec::new(),
        has_filtered_driver: false,
    };
    let manifest_files = rebuild::kept_manifest_files(manifests);
    for driver in registry.drivers() {
        if driver.needs_source_filter() {
            apply_driver_filter(driver, &walk.files, &manifest_files, &mut state)?;
        }
    }
    Ok(state)
}

fn apply_driver_filter(
    driver: &dyn LanguageDriver,
    files: &[Candidate],
    manifest_files: &[ManifestFile],
    state: &mut PrefilterState,
) -> Result<(), IndexError> {
    let exts = driver.language().extensions();
    let candidates: Vec<&Candidate> = files
        .iter()
        .filter(|f| {
            Path::new(&f.path)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .collect();
    if candidates.is_empty() && !has_driver_manifest(manifest_files, driver) {
        return Ok(());
    }
    let (filter_texts, not_utf8) = read_filter_texts(&candidates)?;
    state.filter_not_utf8.extend(not_utf8);
    let Some(res) = driver.filter_sources(manifest_files, &filter_texts) else {
        return Ok(());
    };
    state.has_filtered_driver = true;
    state.build_excluded_count += res.build_excluded_count;
    append_terms(&mut state.build_excluded_terms, &res.build_excluded_terms);
    state.excluded_hashes.extend(res.excluded_hashes);
    state.prefilter_skipped.extend(res.prefilter_skipped);
    state
        .filters
        .insert(driver.language().name().to_owned(), res.kept_paths);
    Ok(())
}

fn append_terms(dst: &mut String, part: &str) {
    if part.is_empty() {
        return;
    }
    if !dst.is_empty() {
        dst.push('\n');
    }
    dst.push_str(part);
}

fn has_driver_manifest(manifests: &[ManifestFile], driver: &dyn LanguageDriver) -> bool {
    manifests.iter().any(|m| {
        Path::new(&m.path)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| driver.manifest_names().contains(&n))
    })
}

type FilterTextPairs = (Vec<(String, String)>, Vec<String>);

fn read_filter_texts(candidates: &[&Candidate]) -> Result<FilterTextPairs, IndexError> {
    let mut out = Vec::with_capacity(candidates.len());
    let mut not_utf8 = Vec::new();
    for file in candidates {
        if let Source::Bytes { bytes, .. } = read_source(&file.abs_path)? {
            match String::from_utf8(bytes) {
                Ok(text) => out.push((file.path.clone(), text)),
                Err(_) => not_utf8.push(file.path.clone()),
            }
        }
    }
    Ok((out, not_utf8))
}

fn index_sources<'a>(
    walk: &'a Walk,
    store: &mut Store,
    registry: &Registry<'_>,
    prefilter: &PrefilterState,
    reextract: bool,
    report: &mut IndexReport,
) -> Result<Vec<&'a Candidate>, IndexError> {
    let mut kept = Vec::new();
    for file in &walk.files {
        let Some(driver) = registry.language_for(&file.path) else {
            continue;
        };
        if !prefilter.allows(driver.language().name(), &file.path) {
            continue;
        }
        index_one_source(file, driver.language(), store, reextract, report, &mut kept)?;
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

pub(crate) fn driver_unread_owned(
    prefilter: &PrefilterState,
    registry: &Registry<'_>,
) -> Vec<(String, Unread)> {
    let mut out = Vec::new();
    for path in &prefilter.filter_not_utf8 {
        if registry.is_supported(path) {
            out.push((path.clone(), Unread::NotUtf8));
        }
    }
    for path in &prefilter.prefilter_skipped {
        if registry.is_supported(path) {
            out.push((path.clone(), Unread::Skipped));
        }
    }
    out
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
    use std::collections::HashMap;

    use phantom_fractal_lang::{Language, ResolveInput, Resolver};
    use phantom_fractal_lang_rust::RustLanguage;

    use super::{DynLang, PrefilterState, RustResolver, driver_unread_owned};
    use crate::fingerprint::Unread;
    use crate::registry::{GoDriver, Registry};

    #[test]
    fn driver_unread_owned_lists_prefiltered_go_paths() {
        let registry = Registry::from_owned(vec![Box::new(GoDriver::new())]);
        let prefilter = PrefilterState {
            filters: HashMap::new(),
            build_excluded_count: 0,
            build_excluded_terms: String::new(),
            excluded_hashes: Vec::new(),
            filter_not_utf8: vec!["internal/zz.go".to_owned()],
            prefilter_skipped: vec!["internal/typo.go".to_owned()],
            has_filtered_driver: true,
        };
        let unread = driver_unread_owned(&prefilter, &registry);
        assert_eq!(unread.len(), 2);
        assert!(
            unread
                .iter()
                .any(|(path, why)| path == "internal/zz.go" && matches!(why, Unread::NotUtf8))
        );
        assert!(
            unread
                .iter()
                .any(|(path, why)| path == "internal/typo.go" && matches!(why, Unread::Skipped))
        );
    }

    #[test]
    fn append_terms_joins_non_empty_lines_only() {
        let mut dst = String::new();
        super::append_terms(&mut dst, "");
        assert_eq!(dst, "");
        super::append_terms(&mut dst, "mod_a\tid1");
        assert_eq!(dst, "mod_a\tid1");
        super::append_terms(&mut dst, "");
        assert_eq!(dst, "mod_a\tid1");
        super::append_terms(&mut dst, "mod_b\tid2");
        assert_eq!(dst, "mod_a\tid1\nmod_b\tid2");
    }

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
