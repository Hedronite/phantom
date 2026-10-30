//! The canonical dump (W9 D6): determinism over rows with no ids, the
//! same graph built in different orders gives the same bytes, a
//! one-byte edit shows, and `index_runs` stays out.

use phantom_fractal_lang::{Module, Target, TargetKind};
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace};
use crate::{CochangeScan, FileRow, PairRow, Store};

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

const ALPHA: &str = "pub fn alpha() -> u8 {\n    1\n}\n";
const USES: &str = "use crate::alpha;\n";

type Source = (&'static str, &'static str);

/// One shared graph: two `.rs` files, a package/target/module, an
/// import link with a live use ref, and one co-change scan.
fn build(store: &mut Store, sources: &[Source]) {
    let language = language();
    for (path, source) in sources.iter().map(|(path, source)| (*path, *source)) {
        store.put_source(path, source, 0, &language).expect("put");
    }
    store
        .write_crate_graph(
            &[crate::Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "Cargo.toml".to_owned(),
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
    drop(language);
}

/// The import link from the stored module through the single use ref,
/// with live ids from the store it builds against.
fn write_stored_links(store: &mut Store) {
    let input = store.resolve_input().expect("resolve");
    let alpha = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "alpha")
        .expect("alpha")
        .id;
    let module = input.modules.first().expect("module").id;
    let use_ref = input
        .uses
        .iter()
        .find(|uses| uses.file == "src/uses.rs")
        .expect("use ref")
        .id;
    store
        .write_links(&[phantom_fractal_lang::Link {
            kind: phantom_fractal_lang::LinkKind::Import,
            source: phantom_fractal_lang::Node::Module(module),
            target: phantom_fractal_lang::Node::Symbol(alpha),
            confidence: 0.95,
            reason: phantom_fractal_lang::LinkReason::Local,
            ref_id: Some(use_ref),
        }])
        .expect("links");
}

/// One co-change scan over the two files, so the pair's stored
/// orientation reads through to paths later.
fn write_stored_scan(store: &mut Store) {
    store
        .write_cochange(&CochangeScan {
            last_oid: Some("aaaa1111".to_owned()),
            oldest_oid: Some("aaaa1111".to_owned()),
            commits_seen: 2,
            skipped_bulk: 0,
            window_json: "{}".to_owned(),
            files: vec![
                FileRow {
                    path: "src/a.rs".to_owned(),
                    commits: 1.0,
                    last_time: 10,
                    renamed_from: None,
                },
                FileRow {
                    path: "src/uses.rs".to_owned(),
                    commits: 2.0,
                    last_time: 20,
                    renamed_from: None,
                },
            ],
            pairs: vec![PairRow {
                a: "src/a.rs".to_owned(),
                b: "src/uses.rs".to_owned(),
                together: 2.0,
                n_a: 1.0,
                n_b: 2.0,
                lift: 1.5,
                conf_ab: 0.4,
                conf_ba: 0.6,
                last_oid: "aaaa1111".to_owned(),
                last_time: 20,
            }],
        })
        .expect("cochange");
}

/// The two put orders for the same sources.
fn forward() -> Vec<Source> {
    [("src/a.rs", ALPHA), ("src/uses.rs", USES)].into()
}

fn backward() -> Vec<Source> {
    [("src/uses.rs", USES), ("src/a.rs", ALPHA)].into()
}

#[test]
fn canonical_dump_of_an_empty_store_is_deterministic_without_id_runs() {
    let db = TempDb::new("dumpempty");
    assert_outside_workspace(db.path());
    let store = Store::open(db.path()).expect("open");
    let first = store.canonical_dump().expect("dump");
    assert!(first.contains("# table files"), "{first}");
    assert!(!first.contains("index_runs"), "{first}");
    // Thirteen canon tables: every canon table but `index_runs`.
    assert_eq!(
        first.matches("\n# table ").count() + (first.starts_with("# table ") as usize),
        13,
        "{first}"
    );
    assert_eq!(first, store.canonical_dump().expect("dump again"));
}

#[test]
fn canonical_dump_is_equal_across_build_orders() {
    let forward_db = TempDb::new("dumpfwd");
    assert_outside_workspace(forward_db.path());
    let mut store = Store::open(forward_db.path()).expect("open forward");
    build(&mut store, &forward());
    write_stored_links(&mut store);
    write_stored_scan(&mut store);
    let forward = store.canonical_dump().expect("forward");
    drop(store);

    let backward_db = TempDb::new("dumpback");
    assert_outside_workspace(backward_db.path());
    let mut store = Store::open(backward_db.path()).expect("open backward");
    build(&mut store, &backward());
    write_stored_links(&mut store);
    write_stored_scan(&mut store);
    let backward = store.canonical_dump().expect("backward");
    drop(store);

    assert_eq!(forward, backward, "the same tree dumps identically");
    // The stored pair carries per-end columns; the dump names them by
    // alphabetical path, so both stores emit the same orientation.
    assert!(
        forward
            .lines()
            .any(|line| { line.starts_with("src/a.rs\tsrc/uses.rs") } && line.contains("1.5")),
        "{forward}"
    );
    // The module end of the import edge is named by its natural key,
    // which embeds the package manifest path and the target.
    assert!(
        forward.lines().any(|line| {
            line.starts_with("import\tmodule\tCargo.toml") && line.contains("alpha")
        }),
        "{forward}"
    );
}

#[test]
fn canonical_dump_shows_a_one_byte_edit() {
    let db = TempDb::new("dumpedit");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    build(&mut store, &forward());
    let baseline = store.canonical_dump().expect("baseline");
    drop(store);
    // Re-open: a strict round trip also dumps the same as its baseline.
    let mut store = Store::open(db.path()).expect("re-open");
    assert_eq!(baseline, store.canonical_dump().expect("stable baseline"));
    let language = language();
    let edited = "pub fn alpha() -> u8 {\n    2\n}\n";
    store
        .put_source("src/a.rs", edited, 0, &language)
        .expect("re-put");
    let after = store.canonical_dump().expect("dump after");
    assert_ne!(baseline, after, "a one-byte edit changes the dump");
}
