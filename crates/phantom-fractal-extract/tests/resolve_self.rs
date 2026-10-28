//! The index pass over `fixtures/resolve-self`, checked against its
//! `EXPECTED.md` (SPEC-w4 G1d).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_extract::{EXTRACTOR_VERSION, stored_graph};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-self");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-self/EXPECTED.md");

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
    assert_eq!(report.unchanged.len(), 3);
    assert_eq!(stored_edges(&s.store), edges);
    assert_eq!(
        stored_graph(&s.store, s.repo.path()).expect("stored graph"),
        first
    );
    assert_eq!(db_digest(&s), before);
}
