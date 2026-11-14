//! The statement error edges (W13 G1): each row statement of a read and
//! of the crate-graph writer fails in turn through the test-only seam,
//! and the call answers a store error the way a real database fault
//! makes it. The seam is `cfg(test)` only, so no non-test build carries
//! it and no behaviour moves.
use super::{TempDb, assert_outside_workspace};
use crate::{Store, fault};
use phantom_fractal_lang::{Module, Target, TargetKind};
use phantom_fractal_lang_rust::RustLanguage;
const ALPHA: &str = "pub fn alpha() -> u8 {\n    1\n}\n";
const USES: &str = "use crate::alpha;\npub fn beta() {\n    alpha();\n}\n";
/// One store with two files, a package, a target, a module and an edge,
/// so every read below has rows to walk before its statement fails.
fn populated(label: &str) -> (TempDb, Store) {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("src/a.rs", ALPHA, 0, &language)
        .expect("put a");
    store
        .put_source("src/b.rs", USES, 0, &language)
        .expect("put b");
    store
        .write_crate_graph(
            &[crate::Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "/w/Cargo.toml".to_owned(),
            }],
            &[Target {
                id: 1,
                package: "sample".to_owned(),
                name: "sample".to_owned(),
                kind: TargetKind::Lib,
                root: "src/a.rs".to_owned(),
            }],
            &[Module {
                id: 1,
                target: 1,
                parent: None,
                path: "crate".to_owned(),
                file: "src/a.rs".to_owned(),
                scope: String::new(),
            }],
            &[],
        )
        .expect("graph");
    store.replace_edges(&[]).expect("edges");
    (db, store)
}
/// Arms each of the call's `statements` row statements in turn: every
/// position must be reached, and the call must answer a store error at
/// each one instead of running on.
fn surfaces(statements: u32, mut call: impl FnMut() -> Result<(), crate::StoreError>) {
    for nth in 1..=statements {
        fault::arm(nth);
        let answered = call();
        assert!(
            !fault::armed(),
            "statement {nth} of the call was never reached"
        );
        assert!(answered.is_err(), "statement {nth} surfaces a store error");
    }
}
#[test]
fn every_read_surfaces_a_failing_statement() {
    let (_db, store) = populated("fault-reads");
    let s = &store;
    // The resolver's five row statements: targets, modules, symbols,
    // uses and call refs fail one at a time.
    surfaces(5, || s.resolve_input().map(|_| ()));
    surfaces(1, || s.symbol_spans().map(|_| ()));
    surfaces(1, || s.symbol_detail(1).map(|_| ()));
    surfaces(1, || s.members(1).map(|_| ()));
    surfaces(1, || s.symbol_facts(1).map(|_| ()));
    surfaces(1, || s.symbol_call_refs(1).map(|_| ()));
    surfaces(1, || s.score_call_refs(1).map(|_| ()));
    surfaces(1, || s.module_rows().map(|_| ()));
    surfaces(1, || s.file_target_counts().map(|_| ()));
    surfaces(1, || s.use_rows().map(|_| ()));
    surfaces(1, || s.import_targets().map(|_| ()));
    surfaces(1, || s.symbol_modules().map(|_| ()));
    surfaces(1, || s.ref_ids_of_kind("call").map(|_| ()));
    surfaces(1, || s.rank_input().map(|_| ()));
    surfaces(1, || s.rank_call_edges().map(|_| ()));
    surfaces(1, || s.rank_import_edges().map(|_| ()));
    surfaces(1, || s.stored_rank().map(|_| ()));
    surfaces(1, || s.files().map(|_| ()));
    surfaces(1, || s.map_rows().map(|_| ()));
    surfaces(1, || s.edges_of_kind("call").map(|_| ()));
    surfaces(1, || s.symbols_matching("alpha").map(|_| ()));
    surfaces(1, || s.callers(1).map(|_| ()));
    surfaces(1, || s.callees(1).map(|_| ()));
    surfaces(1, || s.cochange_of("src/a.rs", 0.0, 5).map(|_| ()));
    // The partner read walks the same statement twice, once per path.
    surfaces(2, || {
        s.cochange_partners_for_files(&["src/a.rs".to_owned(), "src/b.rs".to_owned()], 0.0, 5)
            .map(|_| ())
    });
    surfaces(1, || s.canonical_dump().map(|_| ()));
}
#[test]
fn every_writer_surfaces_a_failing_statement() {
    let (_db, mut store) = populated("fault-writes");
    // The crate-graph writer's package row read fails like any other
    // statement: the write answers an error and the tables it already
    // wrote in this transaction roll back with it.
    surfaces(1, || {
        store.write_crate_graph(&[], &[], &[], &[]).map(|_| ())
    });
    // The failed write rolled back: the module the store already had is
    // still there, and nothing half-written stayed.
    assert_eq!(store.module_rows().expect("the graph still reads").len(), 1);
    // The store still serves every read after the injected faults.
    assert!(store.symbol_spans().expect("spans").len() >= 2);
}
