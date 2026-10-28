//! The index pass over `fixtures/workspace`, checked against its
//! `EXPECTED.md` (SPEC-w4 G2c).

mod common;
mod fixture;

use std::fs;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_extract::{EXTRACTOR_VERSION, stored_graph};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/workspace");
const EXPECTED: &str = include_str!("../../../fixtures/workspace/EXPECTED.md");

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
    assert_eq!(
        s.store.extractor_version().expect("version"),
        Some(EXTRACTOR_VERSION)
    );
}

#[test]
fn a_second_pass_writes_nothing_and_reads_the_same_report_back() {
    let mut s = setup(FIXTURE);
    let first = graph(&run(&mut s));
    let edges = stored_edges(&s.store);
    let before = db_digest(&s);
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert_eq!(report.graph, None);
    assert_eq!(stored_edges(&s.store), edges);
    // The read-back pass scans the same manifests, so `crate-path` calls
    // are not counted as external.
    let again = stored_graph(&s.store, s.repo.path()).expect("stored graph");
    assert_eq!(again, first);
    assert_eq!(again.edges_by_reason.crate_path, 10);
    assert_eq!(db_digest(&s), before);
}

#[test]
fn a_dependency_line_decides_the_edge() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let manifest = s.repo.path().join("uses-ws/Cargo.toml");
    let text = fs::read_to_string(&manifest).expect("read manifest");
    fs::write(
        &manifest,
        text.replace("core-lib.workspace = true", "core-lib = \"0.1.0\""),
    )
    .expect("edit manifest");
    // A manifest-only change rebuilds through the workspace fingerprint.
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    let graph = graph(&report);
    assert_eq!(graph.edges_by_reason.crate_path, 9);
    assert!(
        !stored_edges(&s.store)
            .iter()
            .any(|edge| edge.starts_with("uses-ws/")),
    );
}
