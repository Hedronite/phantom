//! The index pass over `fixtures/resolve-tests`, checked against its
//! `EXPECTED.md` (SPEC-w11 G1b).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-tests");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-tests/EXPECTED.md");

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
}

#[test]
fn test_targets_follow_lib_and_bin_and_all_their_symbols_are_tests() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let input = s.store.resolve_input().expect("resolve input");
    let targets: Vec<String> = input
        .targets
        .iter()
        .map(|t| format!("{:?} {} {}", t.kind, t.name, t.root))
        .collect();
    assert_eq!(
        targets,
        [
            "Lib app app/src/lib.rs",
            "Bin app app/src/main.rs",
            "Lib probe probe/src/lib.rs",
            "Test plain app/tests/plain.rs",
            "Test suite app/tests/suite/main.rs",
        ]
    );
    assert_eq!(
        file_facts(&s.store),
        [
            "app/benches/speed.rs orphan plain",
            "app/examples/demo.rs orphan plain",
            "app/src/lib.rs lib plain",
            "app/src/main.rs bin plain",
            "app/tests/common/mod.rs test test",
            "app/tests/plain.rs test test",
            "app/tests/suite/main.rs test test",
            "probe/src/lib.rs lib plain",
        ]
    );
}

/// `<file> <target kind> test|plain` for each stored symbol, deduplicated.
fn file_facts(store: &Store) -> Vec<String> {
    let input = store.resolve_input().expect("resolve input");
    let mut out: Vec<String> = input
        .symbols
        .iter()
        .map(|symbol| {
            let facts = store
                .symbol_facts(symbol.id)
                .expect("symbol facts")
                .expect("a stored symbol");
            let kind = facts.target_kind.as_deref().unwrap_or("orphan");
            let test = if facts.is_test { "test" } else { "plain" };
            format!("{} {kind} {test}", facts.file)
        })
        .collect();
    out.sort();
    out.dedup();
    out
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
