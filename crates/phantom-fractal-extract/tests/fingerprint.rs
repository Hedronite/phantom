//! The workspace fingerprint decides a rebuild (SPEC-w5 G0b). Each case
//! runs a first pass over a scratch workspace, makes one edit, and runs a
//! second pass.

mod common;
mod fixture;

use std::fs;

use common::{TempDir, rust_bytes, write};
use fixture::{Setup, db_digest, run, stored_edges};
use phantom_fractal_extract::graph::{self, Manifest, SourceFile};
use phantom_fractal_extract::{EXTRACTOR_VERSION, IndexReport, MAX_FILE_BYTES, deps, walk};
use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::{RustLanguage, resolve};
use phantom_fractal_store::Store;

const APP_MANIFEST: &str = "app/Cargo.toml";
const APP_TOML: &str = "[package]\nname = \"app\"\n\n[dependencies]\n";
const DEP_LINE: &str = "base = { path = \"../base\" }\n";

/// A two-member workspace: `app` calls `base::shared`, but its manifest
/// names no dependency yet.
fn scratch() -> Setup {
    let repo = TempDir::new("fingerprint");
    let root = repo.path();
    write(
        root,
        "Cargo.toml",
        b"[workspace]\nmembers = [\"app\", \"base\"]\n",
    );
    write(root, APP_MANIFEST, APP_TOML.as_bytes());
    write(
        root,
        "app/src/lib.rs",
        b"pub fn run() {\n    base::shared();\n}\n",
    );
    write(root, "base/Cargo.toml", b"[package]\nname = \"base\"\n");
    write(root, "base/src/lib.rs", b"pub fn shared() {}\n");
    let db_dir = TempDir::new("db");
    let store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    Setup {
        repo,
        db_dir,
        store,
        rust,
    }
}

fn crate_paths(s: &Setup) -> usize {
    stored_edges(&s.store)
        .iter()
        .filter(|edge| edge.ends_with(" crate-path"))
        .count()
}

/// The second pass rebuilt without writing a source file.
fn assert_rebuilt_alone(report: &IndexReport) {
    assert!(report.graph.is_some(), "no rebuild");
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert!(report.removed.is_empty(), "{:?}", report.removed);
}

#[test]
fn a_new_path_dependency_rebuilds_and_its_revert_rebuilds_again() {
    let mut s = scratch();
    run(&mut s);
    assert_eq!(crate_paths(&s), 0);
    let manifest = s.repo.path().join(APP_MANIFEST);

    fs::write(&manifest, format!("{APP_TOML}{DEP_LINE}")).expect("add dependency");
    let before = db_digest(&s);
    let added = run(&mut s);
    assert_rebuilt_alone(&added);
    assert_eq!(crate_paths(&s), 1);
    assert_eq!(added.graph.expect("graph").edges_by_reason.crate_path, 1);
    assert_ne!(db_digest(&s), before);

    fs::write(&manifest, APP_TOML).expect("revert dependency");
    let reverted = run(&mut s);
    assert_rebuilt_alone(&reverted);
    assert_eq!(crate_paths(&s), 0);
    assert_eq!(reverted.graph.expect("graph").counts.external, 1);
}

#[test]
fn a_whitespace_only_manifest_edit_rebuilds_the_same_edges() {
    let mut s = scratch();
    let manifest = s.repo.path().join(APP_MANIFEST);
    fs::write(&manifest, format!("{APP_TOML}{DEP_LINE}")).expect("add dependency");
    run(&mut s);
    let edges = stored_edges(&s.store);
    let fingerprint = s.store.workspace_fingerprint().expect("fingerprint");
    fs::write(&manifest, format!("{APP_TOML}\n{DEP_LINE}\n")).expect("add blank lines");
    let report = run(&mut s);
    assert_rebuilt_alone(&report);
    assert_eq!(stored_edges(&s.store), edges);
    assert_eq!(crate_paths(&s), 1);
    assert_ne!(
        s.store.workspace_fingerprint().expect("fingerprint"),
        fingerprint
    );
}

#[test]
fn no_change_writes_nothing() {
    let mut s = scratch();
    run(&mut s);
    let before = db_digest(&s);
    let report = run(&mut s);
    assert_eq!(report.graph, None);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert_eq!(db_digest(&s), before);
}

/// What an index pass at the W4 head stored: every kept `.rs` file, the
/// crate graph, the edges, and the extractor version, but no fingerprint
/// and no skip count.
fn index_without_fingerprint(s: &mut Setup) {
    let walk = walk(s.repo.path()).expect("walk");
    let mut manifests = Vec::new();
    let mut files = Vec::new();
    for file in &walk.files {
        let text = fs::read_to_string(&file.abs_path).expect("read");
        if file.path.ends_with("Cargo.toml") {
            manifests.push(Manifest {
                path: file.path.clone(),
                text,
            });
        } else if file.path.ends_with(".rs") {
            s.store
                .put_source(&file.path, &text, 0, &s.rust)
                .expect("put source");
            files.push(SourceFile {
                path: file.path.clone(),
                mod_decls: s.rust.extract(&text).expect("extract").mod_decls,
            });
        }
    }
    let graph = graph::build(&manifests, &files);
    s.store
        .write_crate_graph(&graph.packages, &graph.targets, &graph.modules)
        .expect("crate graph");
    let mut input = s.store.resolve_input().expect("resolve input");
    input.deps = deps::scan(&manifests);
    let (edges, _) = resolve(&input);
    s.store.replace_edges(&edges).expect("edges");
    s.store
        .set_extractor_version(EXTRACTOR_VERSION)
        .expect("version");
}

#[test]
fn a_database_with_no_fingerprint_rebuilds_once_then_is_stable() {
    let mut s = scratch();
    fs::write(
        s.repo.path().join(APP_MANIFEST),
        format!("{APP_TOML}{DEP_LINE}"),
    )
    .expect("add dependency");
    index_without_fingerprint(&mut s);
    let edges = stored_edges(&s.store);
    assert_eq!(crate_paths(&s), 1);
    assert_eq!(s.store.workspace_fingerprint().expect("fingerprint"), None);
    assert_eq!(s.store.rs_skipped().expect("rs skipped"), None);

    let second = run(&mut s);
    assert_rebuilt_alone(&second);
    assert_eq!(second.unchanged.len(), 2);
    assert_eq!(stored_edges(&s.store), edges);
    assert!(
        s.store
            .workspace_fingerprint()
            .expect("fingerprint")
            .is_some()
    );
    assert_eq!(s.store.rs_skipped().expect("rs skipped"), Some(0));

    let before = db_digest(&s);
    let third = run(&mut s);
    assert_eq!(third.graph, None);
    assert!(third.written.is_empty(), "{:?}", third.written);
    assert_eq!(db_digest(&s), before);
}

#[test]
fn a_new_rust_file_over_the_size_cap_rebuilds_and_counts() {
    let mut s = scratch();
    run(&mut s);
    assert_eq!(s.store.rs_skipped().expect("rs skipped"), Some(0));
    write(
        s.repo.path(),
        "app/src/big.rs",
        &rust_bytes(b"pub fn big() {}\n", MAX_FILE_BYTES + 1),
    );
    let report = run(&mut s);
    assert_rebuilt_alone(&report);
    assert_eq!(s.store.rs_skipped().expect("rs skipped"), Some(1));
}

#[test]
fn a_member_manifest_that_is_not_utf8_rebuilds() {
    let mut s = scratch();
    run(&mut s);
    let fingerprint = s.store.workspace_fingerprint().expect("fingerprint");
    write(
        s.repo.path(),
        APP_MANIFEST,
        b"[package]\nname = \"app\"\n# \xff\n",
    );
    let report = run(&mut s);
    assert_rebuilt_alone(&report);
    // `app` has no package now, so its file is an orphan.
    assert_eq!(report.graph.expect("graph").orphans, 1);
    assert_ne!(
        s.store.workspace_fingerprint().expect("fingerprint"),
        fingerprint
    );
}

fn record(out: &mut Vec<u8>, kind: u8, path: &str, tail: &[u8]) {
    out.push(kind);
    out.extend_from_slice(path.as_bytes());
    out.push(0);
    out.extend_from_slice(tail);
}

#[test]
fn the_stored_fingerprint_hashes_the_spec_records_in_walk_order() {
    let mut s = scratch();
    let root = s.repo.path().to_path_buf();
    write(
        &root,
        "app/src/big.rs",
        &rust_bytes(b"pub fn big() {}\n", MAX_FILE_BYTES + 1),
    );
    write(&root, "app/src/bad.rs", b"pub fn bad() {} // \xff\n");
    write(&root, "extra/Cargo.toml", b"# \xfe\n");
    // A skip that is not `.rs` gives no record.
    write(&root, ".env", b"KEY=value\n");
    run(&mut s);

    let hash = |rel: &str| *blake3::hash(&fs::read(root.join(rel)).expect("read")).as_bytes();
    let mut records = Vec::new();
    record(&mut records, b'm', "Cargo.toml", &hash("Cargo.toml"));
    record(&mut records, b'm', APP_MANIFEST, &hash(APP_MANIFEST));
    record(
        &mut records,
        b'm',
        "base/Cargo.toml",
        &hash("base/Cargo.toml"),
    );
    record(&mut records, b'm', "extra/Cargo.toml", b"u");
    // The walk's skip, then the read's.
    record(&mut records, b'r', "app/src/big.rs", b"s");
    record(&mut records, b'r', "app/src/bad.rs", b"u");
    assert_eq!(
        s.store.workspace_fingerprint().expect("fingerprint"),
        Some(*blake3::hash(&records).as_bytes())
    );
    assert_eq!(s.store.rs_skipped().expect("rs skipped"), Some(2));
}
