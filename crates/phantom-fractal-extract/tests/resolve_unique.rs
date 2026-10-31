//! The index pass over `fixtures/resolve-unique`, checked against its
//! `EXPECTED.md` (SPEC-w10 G3a).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-unique");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-unique/EXPECTED.md");

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
}

#[test]
fn a_second_pass_writes_nothing() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let edges = stored_edges(&s.store);
    let before = db_digest(&s);
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert_eq!(report.graph, None);
    assert_eq!(stored_edges(&s.store), edges);
    assert_eq!(db_digest(&s), before);
}
