//! The index pass over `fixtures/resolve-placed`, checked against its
//! `EXPECTED.md` (SPEC-w12 G2a).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_lang::RefKind;
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-placed");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-placed/EXPECTED.md");

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
}

#[test]
fn method_calls_keep_their_receiver_and_hint() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    assert_eq!(method_calls(&s.store), expected_calls(EXPECTED));
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

/// Every stored method call as `caller text receiver hint`, with `-` for
/// none, sorted.
fn method_calls(store: &Store) -> Vec<String> {
    let input = store.resolve_input().expect("resolve input");
    let mut out: Vec<String> = input
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MethodCall)
        .map(|r| {
            let caller = r
                .from
                .and_then(|id| input.symbols.iter().find(|s| s.id == id))
                .map_or("-", |s| s.name.as_str());
            format!(
                "{caller} {} {} {}",
                r.text,
                r.receiver.as_deref().unwrap_or("-"),
                r.receiver_type.as_deref().unwrap_or("-"),
            )
        })
        .collect();
    out.sort();
    out
}

/// The lines of the **Method calls** section, sorted.
fn expected_calls(text: &str) -> Vec<String> {
    let (_, calls) = text
        .split_once("\n## Method calls\n")
        .expect("Method calls section");
    let mut out: Vec<String> = calls
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| line.trim_matches('`').to_owned())
        .collect();
    out.sort();
    out
}
