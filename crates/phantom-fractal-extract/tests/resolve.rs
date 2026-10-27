//! The index pass over `fixtures/resolve`, checked against its
//! `EXPECTED.md` (SPEC-w3 G2c, G2d).

mod common;
mod fixture;

use std::fs;

use fixture::{Setup, assert_matches_expected, expected, graph, row_counts, run, stored_edges};
use phantom_fractal_extract::{EXTRACTOR_VERSION, stored_graph};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve");
const EXPECTED: &str = include_str!("../../../fixtures/resolve/EXPECTED.md");

fn setup() -> Setup {
    fixture::setup(FIXTURE)
}

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup();
    let report = run(&mut s);
    assert_matches_expected(&s.store, &graph(&report), EXPECTED);
    assert_eq!(
        s.store.extractor_version().expect("version"),
        Some(EXTRACTOR_VERSION)
    );
}

#[test]
fn a_second_pass_over_an_unchanged_tree_writes_nothing() {
    let mut s = setup();
    let first = graph(&run(&mut s));
    let edges = stored_edges(&s.store);
    let rows = row_counts(&s.store);
    let db = s.db_dir.path().join("graph.db");
    let before = fs::read(&db).expect("read db");
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert!(report.removed.is_empty(), "{:?}", report.removed);
    assert_eq!(report.graph, None);
    assert_eq!(report.unchanged.len(), 11);
    assert_eq!(row_counts(&s.store), rows);
    assert_eq!(stored_edges(&s.store), edges);
    assert_eq!(fs::read(&db).expect("read db"), before);
    let again = stored_graph(&s.store, s.repo.path()).expect("stored graph");
    assert_eq!(again, first);
    assert_eq!(fs::read(&db).expect("read db"), before);
}

#[test]
fn a_database_at_an_older_extractor_version_is_fully_reextracted() {
    for old in 1..EXTRACTOR_VERSION {
        let mut s = setup();
        let first = run(&mut s);
        s.store.set_extractor_version(old).expect("set version");
        let report = run(&mut s);
        let mut written = report.written.clone();
        written.sort();
        let mut all = first.written.clone();
        all.sort();
        assert_eq!(written.len(), 11, "from version {old}");
        assert_eq!(written, all);
        assert!(report.unchanged.is_empty());
        assert!(report.graph.is_some());
        assert_eq!(
            s.store.extractor_version().expect("version"),
            Some(EXTRACTOR_VERSION)
        );
        assert_eq!(stored_edges(&s.store), expected(EXPECTED).0);
    }
}

#[test]
fn a_deleted_file_loses_its_rows_and_edges() {
    let mut s = setup();
    run(&mut s);
    fs::remove_file(s.repo.path().join("src/util/deep.rs")).expect("delete");
    let report = run(&mut s);
    assert_eq!(report.removed, ["src/util/deep.rs"]);
    assert!(
        !s.store
            .file_paths()
            .expect("paths")
            .contains(&"src/util/deep.rs".to_owned())
    );
    assert!(
        s.store
            .read_items("src/util/deep.rs")
            .expect("items")
            .is_empty()
    );
    let edges = stored_edges(&s.store);
    assert!(!edges.iter().any(|edge| edge.contains("sink")), "{edges:?}");
    let graph = graph(&report);
    assert_eq!(graph.counts.edges, 20);
    assert_eq!(graph.modules, 10);
    // `deep::sink()` in `util::helper` now names a module with no file.
    assert_eq!(graph.counts.unresolved, 9);
}

#[test]
fn an_edited_call_moves_its_edge() {
    let mut s = setup();
    run(&mut s);
    let cli = s.repo.path().join("src/cli.rs");
    let text = fs::read_to_string(&cli).expect("read cli");
    fs::write(
        &cli,
        text.replace("    banner(); // local\n", "    crate::cli::banner();\n"),
    )
    .expect("edit cli");
    let report = run(&mut s);
    assert_eq!(report.written, ["src/cli.rs"]);
    let edges = stored_edges(&s.store);
    assert!(
        edges.contains(&"main:crate::cli::run -> main:crate::cli::banner path-exact".to_owned())
    );
    assert!(!edges.contains(&"main:crate::cli::run -> main:crate::cli::banner local".to_owned()));
}
