//! The index pass over `fixtures/resolve-nested`, checked against its
//! `EXPECTED.md` (SPEC-w16 G2).

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};
use phantom_fractal_extract::{EXTRACTOR_VERSION, symbol_keys};
use phantom_fractal_lang::Language;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-nested");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-nested/EXPECTED.md");

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
fn second_pass_over_resolve_nested_writes_nothing() {
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

#[test]
fn nested_fns_carry_distinct_stable_keys_and_reextract_from_v13() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let input = s.store.resolve_input().expect("resolve input");
    let shared: Vec<_> = input
        .symbols
        .iter()
        .filter(|sym| sym.name == "shared_helper")
        .collect();
    assert_eq!(shared.len(), 2, "{shared:?}");
    let owners: Vec<_> = shared.iter().map(|sym| sym.owner.as_deref()).collect();
    assert_eq!(owners, [Some("outer_alpha"), Some("outer_beta")]);

    for key in [
        "src/app.rs#nearby_entries::is_root#function#0",
        "src/app.rs#App::run::step#function#0",
        "src/lib.rs#outer_alpha::shared_helper#function#0",
        "src/lib.rs#outer_beta::shared_helper#function#0",
        "src/lib.rs#layered::mid#function#0",
        "src/lib.rs#layered::mid::leaf#function#0",
    ] {
        assert!(
            s.store.symbol_by_stable_key(key).expect("lookup").is_some(),
            "missing stable key {key}"
        );
    }

    s.store.set_extractor_version(13).expect("downgrade to v13");
    let rerun = run(&mut s);
    assert_eq!(rerun.written.len(), 2);
    assert_eq!(
        s.store.extractor_version().expect("version"),
        Some(EXTRACTOR_VERSION)
    );
}

#[test]
fn symbol_keys_distinguish_nested_owners_kinds_and_ordinals() {
    let rust = phantom_fractal_lang_rust::RustLanguage::new().expect("lang");
    let src = "struct dup;\nfn dup() {}\nfn a() { fn dup() {} }\nfn b(c: bool) { if c { fn dup() {} } else { fn dup() {} } }\n";
    let ext = rust.extract(src).expect("extract");
    let keys: Vec<String> = symbol_keys("src/k.rs", &ext)
        .into_iter()
        .map(|(k, _, _, _)| k)
        .collect();
    assert_eq!(
        keys,
        [
            "src/k.rs#dup#struct#0",
            "src/k.rs#dup#function#0",
            "src/k.rs#a#function#0",
            "src/k.rs#a::dup#function#0",
            "src/k.rs#b#function#0",
            "src/k.rs#b::dup#function#0",
            "src/k.rs#b::dup#function#1",
        ]
    );
}
