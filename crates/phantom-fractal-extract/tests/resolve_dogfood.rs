//! The index pass over `fixtures/resolve-dogfood`, checked against its
//! `EXPECTED.md` (SPEC-w13 G2a): the D9 cures and the cases they leave.

mod common;
mod fixture;

use fixture::{assert_matches_expected, db_digest, graph, run, setup, stored_edges};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/resolve-dogfood"
);
const EXPECTED: &str = include_str!("../../../fixtures/resolve-dogfood/EXPECTED.md");

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

/// The store keeps one file row and one symbol row for the shared file,
/// and every read that picks a file's module takes its first module row:
/// the module of its symbols, of the `use` and call rows of the file, and
/// the module a name query reports (SPEC-w13 G2a, D9 (a), R13).
#[test]
fn the_shared_file_keeps_one_row_each_and_its_first_module() {
    let mut s = setup(FIXTURE);
    run(&mut s);
    let input = s.store.resolve_input().expect("resolve input");
    let rows: Vec<&phantom_fractal_lang::Module> = input
        .modules
        .iter()
        .filter(|module| module.file == "tests/common/mod.rs")
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let (first, later) = (rows[0], rows[1]);
    assert!(first.id < later.id);
    let targets: Vec<&str> = [first, later]
        .iter()
        .map(|module| {
            input
                .targets
                .iter()
                .find(|target| target.id == module.target)
                .map_or("", |target| target.root.as_str())
        })
        .collect();
    assert_eq!(targets, ["tests/first.rs", "tests/second.rs"]);
    let shared: Vec<&phantom_fractal_lang::Symbol> = input
        .symbols
        .iter()
        .filter(|symbol| symbol.file == "tests/common/mod.rs")
        .collect();
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert_eq!(shared[0].module, Some(first.id));
    let second_root = input
        .modules
        .iter()
        .find(|module| module.file == "tests/second.rs" && module.parent.is_none())
        .expect("second root");
    let glob = input
        .uses
        .iter()
        .find(|binding| binding.file == "tests/second.rs" && binding.glob)
        .expect("the glob");
    assert_eq!(glob.module, Some(second_root.id));
    let found = s
        .store
        .symbols_matching("common::fixture_data")
        .expect("query");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].module_path, "crate::common");
    assert_eq!(found[0].file, "tests/common/mod.rs");
    let files = s.store.file_paths().expect("paths");
    let at = files
        .iter()
        .filter(|path| path.as_str() == "tests/common/mod.rs")
        .count();
    assert_eq!(at, 1);
}
