//! `fixtures/resolve-typed`, checked against its `EXPECTED.md` (SPEC-w10
//! G2a).
//!
//! Until the store carries `refs.receiver_hint` and `symbols.signature`
//! into the resolver (G2b), an index pass places no `typed-method` edge.
//! So the typed lines are checked on the store's resolve input with the
//! hints and signatures filled from the extractor, as G2b fills them, and
//! the index pass is checked on every other line. The full end-to-end test
//! is G2b's acceptance.

mod common;
mod fixture;

use std::collections::HashMap;
use std::fs;

use fixture::{Setup, assert_matches_expected, expected, graph, labels, run, setup, stored_edges};
use phantom_fractal_extract::stored_input;
use phantom_fractal_lang::{Language, Reason, ResolveInput};
use phantom_fractal_lang_rust::{confidence, resolve_graph};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve-typed");
const EXPECTED: &str = include_str!("../../../fixtures/resolve-typed/EXPECTED.md");
const TYPED: &str = " typed-method";

/// The resolver input of the indexed copy with each ref's receiver hint and each
/// symbol's signature taken from the extract of its file. Rows of a file
/// are in extract order, items before methods, so they pair one to one.
fn hinted_input(s: &Setup) -> ResolveInput {
    let mut input = stored_input(&s.store, s.repo.path()).expect("resolve input");
    let mut files: Vec<String> = input.refs.iter().map(|r| r.file.clone()).collect();
    files.extend(input.symbols.iter().map(|symbol| symbol.file.clone()));
    files.sort();
    files.dedup();
    for file in files {
        let text = fs::read_to_string(s.repo.path().join(&file)).expect("read source");
        let extract = s.rust.extract(&text).expect("extract");
        let refs = input.refs.iter_mut().filter(|r| r.file == file);
        let mut count = 0;
        for (call, reference) in refs.zip(&extract.refs) {
            assert_eq!(
                (call.kind, &call.text),
                (reference.kind, &reference.text),
                "{file}"
            );
            call.receiver_type = reference.receiver_type.clone();
            count += 1;
        }
        assert_eq!(count, extract.refs.len(), "{file}");
        let declared = extract
            .items
            .iter()
            .map(|item| (&item.name, &item.signature))
            .chain(
                extract
                    .methods
                    .iter()
                    .map(|method| (&method.name, &method.signature)),
            );
        let symbols = input
            .symbols
            .iter_mut()
            .filter(|symbol| symbol.file == file);
        let mut count = 0;
        for (symbol, (name, signature)) in symbols.zip(declared) {
            assert_eq!(&symbol.name, name, "{file}");
            symbol.signature = Some(signature.clone());
            count += 1;
        }
        assert_eq!(count, extract.items.len() + extract.methods.len(), "{file}");
    }
    input
}

#[test]
fn hinted_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = graph(&run(&mut s));
    let input = hinted_input(&s);
    let names = labels(&s.store, &input);
    let resolved = resolve_graph(&input);
    let mut edges: Vec<String> = resolved
        .calls
        .iter()
        .map(|edge| {
            assert_eq!(edge.confidence, confidence(edge.reason), "{edge:?}");
            format!(
                "{} -> {} {}",
                names[&edge.source],
                names[&edge.target],
                edge.reason.as_str()
            )
        })
        .collect();
    edges.sort();
    let (want, counts) = expected(EXPECTED);
    assert_eq!(edges, want);
    let typed = resolved
        .calls
        .iter()
        .filter(|edge| edge.reason == Reason::TypedMethod)
        .inspect(|edge| assert_eq!(edge.confidence, 0.8))
        .count();
    assert!(typed > 0);
    let got = HashMap::from([
        ("edges", resolved.counts.edges),
        ("unresolved", resolved.counts.unresolved),
        ("external", resolved.counts.external),
        ("no_source", resolved.counts.no_source),
        ("method_calls", resolved.counts.method_calls),
        ("macros", resolved.counts.macros),
        ("modules", report.modules as u64),
        ("orphans", report.orphans as u64),
    ]);
    assert_eq!(got.len(), counts.len());
    for (key, value) in &counts {
        assert_eq!(got.get(key.as_str()), Some(value), "{key}");
    }
}

/// Every line but the `typed-method` ones is stored by an index pass, and
/// each typed call is an edge or a method call. This holds before the
/// store carries the hint (G2b), when no typed edge is placed, and after.
#[test]
fn an_index_pass_places_every_other_line() {
    let mut s = setup(FIXTURE);
    let report = graph(&run(&mut s));
    let (want, counts) = expected(EXPECTED);
    let others: Vec<String> = want
        .iter()
        .filter(|edge| !edge.ends_with(TYPED))
        .cloned()
        .collect();
    let stored: Vec<String> = stored_edges(&s.store)
        .into_iter()
        .filter(|edge| !edge.ends_with(TYPED))
        .collect();
    assert_eq!(stored, others);
    let typed = report.edges_by_reason.typed_method;
    assert_eq!(report.counts.edges, others.len() as u64 + typed);
    assert_eq!(
        report.counts.edges + report.counts.method_calls,
        counts["edges"] + counts["method_calls"]
    );
    assert_eq!(report.counts.unresolved, counts["unresolved"]);
    assert_eq!(report.counts.external, counts["external"]);
}

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
}
