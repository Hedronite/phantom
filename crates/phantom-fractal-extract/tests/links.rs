//! The `import` and `impl` links of `fixtures/graph`, checked against its
//! `EXPECTED.md` (SPEC-w7 G0a).

mod common;
mod fixture;

use fixture::{Setup, db_digest, labels, module_labels, run, setup};
use phantom_fractal_extract::{EXTRACTOR_VERSION, stored_links};
use phantom_fractal_lang::{LinkKind, LinkReason, Node};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/graph");
const EXPECTED: &str = include_str!("../../../fixtures/graph/EXPECTED.md");

/// The rows of the **Import edges** and **Impl edges** sections, sorted.
fn expected() -> Vec<String> {
    let mut rows = Vec::new();
    let mut section = "";
    for line in EXPECTED.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading;
            continue;
        }
        if let ("Import edges" | "Impl edges", Some(row)) = (section, line.strip_prefix("- ")) {
            rows.push(row.trim_matches('`').to_owned());
        }
    }
    rows.sort();
    rows
}

/// Every link of the indexed fixture as `kind source -> target reason
/// confidence`, sorted. Each node is written `module <label>` or `symbol
/// <label>`. Each link's `ref_id` and reason are checked against its kind.
fn links(s: &Setup) -> Vec<String> {
    let input = s.store.resolve_input().expect("resolve input");
    let symbols = labels(&s.store, &input);
    let modules = module_labels(&input);
    let node = |node: Node| match node {
        Node::Module(id) => format!("module {}", modules[&id]),
        Node::Symbol(id) => format!("symbol {}", symbols[&id]),
    };
    let mut out = Vec::new();
    for link in stored_links(&s.store, s.repo.path()).expect("links") {
        match link.kind {
            LinkKind::Import => {
                let binding = input
                    .uses
                    .iter()
                    .find(|binding| Some(binding.id) == link.ref_id)
                    .expect("an import's ref is its use");
                assert!(!binding.glob, "{link:?}");
                assert!(matches!(link.source, Node::Module(_)), "{link:?}");
                assert_ne!(link.reason, LinkReason::ImplTrait, "{link:?}");
            }
            LinkKind::Impl => {
                assert_eq!(link.ref_id, None, "{link:?}");
                assert_eq!(link.reason, LinkReason::ImplTrait, "{link:?}");
            }
        }
        out.push(format!(
            "{} {} -> {} {} {:.2}",
            link.kind.as_str(),
            node(link.source),
            node(link.target),
            link.reason.as_str(),
            link.confidence
        ));
    }
    out.sort();
    out
}

#[test]
fn fixture_links_match_expected_exactly() {
    let mut s = setup(FIXTURE);
    let report = run(&mut s);
    assert!(report.graph.is_some(), "the first pass rebuilds");
    assert_eq!(links(&s), expected());
    assert_eq!(
        s.store.extractor_version().expect("version"),
        Some(EXTRACTOR_VERSION)
    );
}

#[test]
fn expected_lists_both_kinds_and_no_duplicate_row() {
    let rows = expected();
    let mut unique = rows.clone();
    unique.dedup();
    assert_eq!(unique, rows);
    assert!(rows.iter().any(|row| row.starts_with("import ")));
    assert!(rows.iter().any(|row| row.starts_with("impl ")));
}

#[test]
fn a_second_pass_writes_nothing_and_reads_the_same_links_back() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let first = links(&s);
    let before = db_digest(&s);
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert_eq!(report.graph, None);
    assert_eq!(links(&s), first);
    assert_eq!(db_digest(&s), before);
}
