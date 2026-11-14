//! The index pass over `fixtures/resolve-macros`, checked against its
//! `EXPECTED.md` (SPEC-w12 G1b).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_lang::RefKind;
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-macros");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-macros/EXPECTED.md");

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
}

#[test]
fn macro_arg_refs_keep_their_receiver_hint_and_shadowing() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    assert_eq!(macro_args(&s.store), expected_refs(EXPECTED));
}

#[test]
fn a_macro_rules_body_gives_no_ref() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let input = s.store.resolve_input().expect("resolve input");
    assert!(
        input.refs.iter().all(|r| r.text != "hidden_in_body"),
        "{:?}",
        input.refs
    );
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

/// Every stored `macro_arg` ref as `caller text receiver hint shadowed`,
/// with `-` for none, sorted.
fn macro_args(store: &Store) -> Vec<String> {
    let input = store.resolve_input().expect("resolve input");
    let mut out: Vec<String> = input
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MacroArg)
        .map(|r| {
            let caller = r
                .from
                .and_then(|id| input.symbols.iter().find(|s| s.id == id))
                .map_or("-", |s| s.name.as_str());
            format!(
                "{caller} {} {} {} {}",
                r.text,
                r.receiver.as_deref().unwrap_or("-"),
                r.receiver_type.as_deref().unwrap_or("-"),
                r.shadowed
            )
        })
        .collect();
    out.sort();
    out
}

/// The lines of the **Refs** section, sorted.
fn expected_refs(text: &str) -> Vec<String> {
    let (_, refs) = text.split_once("\n## Refs\n").expect("Refs section");
    let mut out: Vec<String> = refs
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| line.trim_matches('`').to_owned())
        .collect();
    out.sort();
    out
}
