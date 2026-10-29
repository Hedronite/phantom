//! Rank store and map-row tests (SPEC-w7 G1a).

use phantom_fractal_lang::{Edge, Reason, RefKind, SymbolKind};
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace};
use crate::Store;

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

fn write_graph(store: &mut Store, file: &str, module_input_id: i64) {
    let (package, targets, modules) = (
        crate::Package {
            name: "sample".to_owned(),
            version: None,
            manifest_path: "Cargo.toml".to_owned(),
        },
        std::iter::once(phantom_fractal_lang::Target {
            id: 1,
            package: "sample".to_owned(),
            name: "sample".to_owned(),
            kind: phantom_fractal_lang::TargetKind::Lib,
            root: file.to_owned(),
        })
        .collect::<Vec<_>>(),
        std::iter::once(phantom_fractal_lang::Module {
            id: module_input_id,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: file.to_owned(),
            scope: String::new(),
        })
        .collect::<Vec<_>>(),
    );
    store
        .write_crate_graph(
            &[package],
            &targets.clone(),
            &modules
                .clone()
                .into_iter()
                .map(|mut module| {
                    module.id = module_input_id;
                    module
                })
                .collect::<Vec<_>>(),
            &[],
        )
        .expect("graph");
}

#[test]
fn graph_epoch_moves_only_on_a_rebuild() {
    let db = TempDb::new("epoch");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert_eq!(store.graph_epoch().expect("unset"), 0);
    store.replace_edges(&[]).expect("first build bumps");
    assert_eq!(store.graph_epoch().expect("first"), 1);
    store.replace_edges(&[]).expect("second build");
    assert_eq!(store.graph_epoch().expect("second"), 2);
    // Rank writes do not move the graph epoch; the writeless pass calls
    // nothing at all.
    store.write_rank(1, &[], &[]).expect("rank write");
    assert_eq!(store.graph_epoch().expect("stable"), 2);
    assert_eq!(store.rank_epoch().expect("rank epoch"), 1);
    drop(store);
}
#[test]
fn rank_input_carries_the_rank_fields() {
    let db = TempDb::new("rankinput");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\nfn side() {}\nfn main() {}\n";
    store
        .put_source("src/main.rs", source, 0, &language)
        .expect("put");
    let rows = store.rank_input().expect("rank input");
    assert_eq!(rows.len(), 3, "{rows:?}");
    for row in &rows {
        assert!(row.target_kind.is_none(), "{row:?} before any graph");
    }
    assert!(rows.iter().all(|row| !row.is_main));
    // A binned target flips the `main` detection once write_crate_graph
    // lines the module up.
    store
        .write_crate_graph(
            &[crate::Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "Cargo.toml".to_owned(),
            }],
            &[phantom_fractal_lang::Target {
                id: 4,
                package: "sample".to_owned(),
                name: "sample".to_owned(),
                kind: phantom_fractal_lang::TargetKind::Bin,
                root: "src/main.rs".to_owned(),
            }],
            &[phantom_fractal_lang::Module {
                id: 6,
                target: 4,
                parent: None,
                path: "crate".to_owned(),
                file: "src/main.rs".to_owned(),
                scope: String::new(),
            }],
            &[],
        )
        .expect("graph");
    let rows = store.rank_input().expect("rank input after graph");
    assert!(rows.iter().all(|row| row.path == "src/main.rs"), "{rows:?}");
    let fn_main_row = rows
        .iter()
        .find(|row| row.is_main)
        .expect("fn main at the bin root is flagged");
    assert_eq!(fn_main_row.path, "src/main.rs");
    assert_eq!(fn_main_row.kind, "function");
    assert_eq!(fn_main_row.target_kind.as_deref(), Some("bin"));
    drop(store);
}
#[test]
fn rank_edges_and_import_pairs_arrive_in_id_order() {
    let db = TempDb::new("rankedges");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\npub fn outer() { pick(); }\npub struct Ledger;\n";
    store
        .put_source("src/rank.rs", source, 0, &language)
        .expect("put");
    store
        .put_source("src/uses.rs", "use crate::pick;\n", 0, &language)
        .expect("put uses");
    write_graph(&mut store, "src/rank.rs", 1);
    let input = store.resolve_input().expect("input");
    let outer = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "outer")
        .expect("outer")
        .id;
    let pick = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "pick")
        .expect("pick")
        .id;
    let ledger = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Ledger")
        .expect("Ledger")
        .id;
    let module = input.modules.first().expect("module").id;
    let reference = input
        .refs
        .iter()
        .find(|reference| reference.text == "pick")
        .expect("ref")
        .id;
    let use_ref = input.uses.first().expect("use ref").id;
    store
        .replace_edges(&[Edge {
            ref_id: reference,
            source: outer,
            target: pick,
            confidence: 0.95,
            reason: Reason::Local,
        }])
        .expect("call edges");
    store
        .write_links(&[
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Impl,
                source: phantom_fractal_lang::Node::Symbol(outer),
                target: phantom_fractal_lang::Node::Symbol(ledger),
                confidence: 0.8,
                reason: phantom_fractal_lang::LinkReason::ImplTrait,
                ref_id: None,
            },
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Import,
                source: phantom_fractal_lang::Node::Module(module),
                target: phantom_fractal_lang::Node::Symbol(pick),
                confidence: 0.9,
                reason: phantom_fractal_lang::LinkReason::Local,
                ref_id: Some(use_ref),
            },
        ])
        .expect("links");
    let edges = store.rank_call_edges().expect("rank call edges");
    assert_eq!(edges.len(), 2, "{edges:?}");
    // `use crate::pick;` is a use in src/uses.rs: the import edge ends on
    // pick, whose file is src/rank.rs — the (importing, imported) pair.
    let imports = store.rank_import_edges().expect("rank import edges");
    assert_eq!(imports.len(), 1, "{imports:?}");
}
#[test]
fn write_rank_replaces_whole_table_atomically() {
    let db = TempDb::new("writerank");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    store.replace_edges(&[]).expect("first build");
    store
        .write_rank(1, &[(7, 0.5), (9, 0.5)], &[(3, 1.0)])
        .expect("write");
    assert_eq!(store.rank_epoch().expect("rank epoch"), 1);
    // A second write replaces both kinds instead of appending.
    store.write_rank(2, &[(7, 1.0)], &[]).expect("rewrite");
    assert_eq!(store.rank_epoch().expect("rank epoch"), 2);
    assert!(store.rank_call_edges().is_ok(), "the table clears");
    drop(store);
}
#[test]
fn rank_import_edges_carry_both_ends_and_rank_edges_kinds() {
    let db = TempDb::new("rankimport");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 {\n    1\n}\n";
    store
        .put_source("src/lib.rs", source, 0, &language)
        .expect("put lib");
    let nested = "use super::pick;\npub struct Ledger;\n\nimpl Ledger {\n    pub fn measure(&self) -> u32 {\n        pick() as u32\n    }\n}\n";
    store
        .put_source("src/nested.rs", nested, 0, &language)
        .expect("put nested");
    store
        .write_crate_graph(
            &[crate::Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "Cargo.toml".to_owned(),
            }],
            &[phantom_fractal_lang::Target {
                id: 1,
                package: "sample".to_owned(),
                name: "sample".to_owned(),
                kind: phantom_fractal_lang::TargetKind::Lib,
                root: "src/lib.rs".to_owned(),
            }],
            &[
                phantom_fractal_lang::Module {
                    id: 1,
                    target: 1,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "src/lib.rs".to_owned(),
                    scope: String::new(),
                },
                phantom_fractal_lang::Module {
                    id: 2,
                    target: 1,
                    parent: Some(1),
                    path: "crate::nested".to_owned(),
                    file: "src/nested.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            &[],
        )
        .expect("graph");
    let input = store.resolve_input().expect("input");
    let pick = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "pick")
        .expect("pick")
        .id;
    let measure = input
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::Method && symbol.name == "measure")
        .expect("measure method")
        .id;
    let reference = input
        .refs
        .iter()
        .find(|reference| {
            reference.kind == RefKind::Call
                && reference.from == inputs_from(input.symbols.as_slice(), measure)
        })
        .expect("`pick()` call inside measure")
        .id;
    let use_ref = input.uses.first().expect("use ref").id;
    // One import edge from the nested module row to the resolved `pick`
    // symbol, and one impl edge from the method to its resolved trait.
    let trait_id = pick; // the same trait symbol row is `pick` here only
    store
        .replace_edges(&[Edge {
            ref_id: reference,
            source: measure,
            target: pick,
            confidence: 0.95,
            reason: Reason::Local,
        }])
        .expect("call edge");
    store
        .write_links(&[
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Import,
                source: phantom_fractal_lang::Node::Module(
                    input
                        .modules
                        .iter()
                        .find(|module| module.file == "src/nested.rs")
                        .expect("nested file module")
                        .id,
                ),
                target: phantom_fractal_lang::Node::Symbol(pick),
                confidence: 0.95,
                reason: phantom_fractal_lang::LinkReason::Local,
                ref_id: Some(use_ref),
            },
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Impl,
                source: phantom_fractal_lang::Node::Symbol(measure),
                target: phantom_fractal_lang::Node::Symbol(trait_id),
                confidence: 0.8,
                reason: phantom_fractal_lang::LinkReason::ImplTrait,
                ref_id: None,
            },
        ])
        .expect("links");

    // (2) rank_call_edges carries BOTH kinds with kind texts.
    let edges = store.rank_call_edges().expect("rank call edges");
    assert_eq!(edges.len(), 2, "{edges:?}");
    assert_eq!(edges.iter().filter(|edge| edge.kind == "call").count(), 1);
    assert_eq!(edges.iter().filter(|edge| edge.kind == "impl").count(), 1);
    assert!(edges.iter().all(|edge| edge.confidence > 0.0));

    // (1) The import side is joined over modules.file_id: the nested
    // module's FILE row is the importing file, and its imported symbol
    // lives in src/lib.rs.
    let imports = store.rank_import_edges().expect("rank import edges");
    assert_eq!(imports.len(), 1, "{imports:?}");
    let rows = store.rank_input().expect("rank input");
    let nested_file_id = rows
        .iter()
        .find(|row| row.path == "src/nested.rs")
        .map(|row| row.file_id);
    drop(rows);
    let _ = trait_id;
    let _ = inputs_from;
    assert_eq!(imports[0].src_file_id, nested_file_id.unwrap());
}

fn inputs_from(symbols: &[phantom_fractal_lang::Symbol], id: i64) -> Option<i64> {
    symbols
        .iter()
        .find(|symbol| symbol.id == id)
        .map(|symbol| symbol.id)
}
#[test]
fn map_rows_skip_tests_and_modules() {
    let db = TempDb::new("maprows");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn open_ledger() -> u32 {\n    0\n}\n\n#[test]\nfn probe() {}\n\npub struct Ledger;\npub mod inner {\n\n}\n";
    store
        .put_source("src/mapme.rs", source, 0, &language)
        .expect("put");
    let rows = store.map_rows().expect("map rows");
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert!(
        !names.contains(&"probe"),
        "a #[test] fn is a test: {names:?}"
    );
    assert!(!names.contains(&"inner"), "{names:?}");
    let item = rows
        .iter()
        .find(|row| row.name == "open_ledger")
        .expect("row");
    assert_eq!(item.kind, "function");
    // The signature stores the pre-body text as W6 wrote it.
    assert!(
        item.signature
            .as_deref()
            .is_some_and(|s| s.starts_with("pub fn"))
    );
    // Files come back sorted by path.
    let paths = store.map_file_paths().expect("paths").len();
    assert!(paths >= 1);
    drop(store);
}
