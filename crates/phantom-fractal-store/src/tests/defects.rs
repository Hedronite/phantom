//! The arms a malformed store reaches (W13 G1): a path that is data
//! rather than framing, and references whose target row left. The
//! canonical dump must escape the first and name the second, and it
//! must do both deterministically.
use super::{TempDb, assert_outside_workspace};
use crate::Store;
use phantom_fractal_lang_rust::RustLanguage;
const ALPHA: &str = "pub fn alpha() -> u8 {\n    1\n}\n";
/// A path carrying every character the dump escapes, plus two control
/// bytes a path may legally hold on a Unix filesystem.
const ODD: &str = "src/odd\tname\nline\rcarry\\slash\u{1}ctl\u{7f}del.rs";
#[test]
fn canonical_dump_escapes_its_cells_and_names_what_it_cannot_resolve() {
    let db = TempDb::new("defects-dump");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store.put_source(ODD, ALPHA, 0, &language).expect("put odd");
    store
        .put_source("src/a.rs", ALPHA, 0, &language)
        .expect("put a");
    // Rows whose references left: a target over a package the table
    // does not hold, a module over a target it does not hold, and rank
    // rows over a symbol it does not hold, over a package id, and under
    // a node kind the dump does not resolve.
    store
        .conn
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO targets(package_id, kind, name, root_file)
                 VALUES (999999, 'lib', 'ghost', 'src/a.rs');
             INSERT INTO modules(target_id, path, file_id, scope)
                 VALUES (999999, 'crate', NULL, '');
             INSERT INTO rank(node_kind, node_id, score, epoch) VALUES ('symbol', 999999, 1.0, 1);
             INSERT INTO rank(node_kind, node_id, score, epoch) VALUES ('package', 1, 2.0, 1);
             INSERT INTO rank(node_kind, node_id, score, epoch) VALUES ('bogus', 1, 3.0, 1);
             INSERT INTO edges(kind, src_kind, src_id, dst_kind, dst_id, confidence, reason, ref_id)
                 VALUES ('call', 'symbol', 1, 'symbol', 999999, 0.5, 'local', NULL);",
        )
        .expect("poison");
    let dump = store.canonical_dump().expect("dump");
    // The escapes: tab, newline, carriage return, backslash, and the two
    // control characters, so one stored path stays one cell.
    assert!(
        dump.contains("src/odd\\tname\\nline\\rcarry\\\\slash\\x01ctl\\x7fdel.rs"),
        "{dump}"
    );
    // A reference with no row to name reads its dangling marker.
    assert!(dump.contains("\\N"), "{dump}");
    // The framing holds: thirteen tables, one header each.
    assert_eq!(dump.matches("\n# table ").count() + 1, 13, "{dump}");
    // And the abnormal rows do not make the dump unstable.
    assert_eq!(dump, store.canonical_dump().expect("dump again"));
}
/// Every canon table one store needs, minus the two the lock and the
/// stamp keep.
const TABLES: &[&str] = &[
    "packages",
    "targets",
    "modules",
    "symbols",
    "refs",
    "edges",
    "files",
    "file_history",
    "git_scan",
    "cochange",
    "rank",
];
/// One store with rows in every table, then `table` dropped.
fn store_missing(label: &str, table: &str) -> (TempDb, Store) {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("src/a.rs", ALPHA, 0, &language)
        .expect("put a");
    store
        .put_source(
            "src/b.rs",
            "use crate::alpha;\npub fn beta() {\n    alpha();\n}\n",
            0,
            &language,
        )
        .expect("put b");
    let package = crate::Package {
        name: "sample".to_owned(),
        version: None,
        manifest_path: "/w/Cargo.toml".to_owned(),
    };
    let target = phantom_fractal_lang::Target {
        id: 1,
        package: "sample".to_owned(),
        name: "sample".to_owned(),
        kind: phantom_fractal_lang::TargetKind::Lib,
        root: "src/a.rs".to_owned(),
    };
    let module = phantom_fractal_lang::Module {
        id: 1,
        target: 1,
        parent: None,
        path: "crate".to_owned(),
        file: "src/a.rs".to_owned(),
        scope: String::new(),
    };
    store
        .write_crate_graph(&[package], &[target], &[module], &[])
        .expect("graph");
    store.replace_edges(&[]).expect("edges");
    store.write_rank(1, &[], &[]).expect("rank");
    store
        .conn
        .execute_batch(
            "DROP TRIGGER IF EXISTS symbols_ai;
             DROP TRIGGER IF EXISTS symbols_ad;
             DROP TRIGGER IF EXISTS symbols_au;
             PRAGMA foreign_keys = OFF;",
        )
        .expect("prep");
    store
        .conn
        .execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\";"))
        .expect("drop");
    (db, store)
}
#[test]
fn every_read_answers_when_a_table_is_missing() {
    for table in TABLES {
        let (_db, store) = store_missing(&format!("missing-{table}"), table);
        // Every read this crate exposes answers — a value or a store
        // error — instead of panicking on the missing table.
        let _ = store.file_paths();
        let _ = store.file_stats();
        let _ = store.read_items("src/a.rs");
        let _ = store.stored_blake3("src/a.rs");
        let _ = store.ref_texts("src/a.rs");
        let _ = store.match_count("alpha");
        let _ = store.producer();
        let _ = store.schema_version();
        let _ = store.user_version();
        let _ = store.row_count("files");
        let _ = store.read_only_still_valid();
        let _ = store.trusted_schema_is_off();
        let _ = store.synchronous_is_normal();
        let _ = store.defensive_is_on();
        let _ = store.symbol_spans();
        let _ = store.symbol_detail(1);
        let _ = store.symbol_facts(1);
        let _ = store.members(1);
        let _ = store.symbol_by_stable_key("k");
        let _ = store.symbol_call_refs(1);
        let _ = store.unresolved_named("alpha");
        let _ = store.macro_args_named("alpha");
        let _ = store.file_has_module(1);
        let _ = store.score_call_refs(1);
        let _ = store.module_rows();
        let _ = store.use_rows();
        let _ = store.import_targets();
        let _ = store.package_dirs();
        let _ = store.symbol_modules();
        let _ = store.ref_ids_of_kind("call");
        let _ = store.file_target_counts();
        let _ = store.rank_input();
        let _ = store.rank_call_edges();
        let _ = store.rank_import_edges();
        let _ = store.stored_rank();
        let _ = store.files();
        let _ = store.map_file_paths();
        let _ = store.map_rows();
        let _ = store.graph_epoch();
        let _ = store.rank_epoch();
        let _ = store.workspace_fingerprint();
        let _ = store.rs_skipped();
        let _ = store.extractor_version();
        let _ = store.edges_of_kind("call");
        let _ = store.resolve_input();
        let _ = store.symbols_matching("alpha");
        let _ = store.callers(1);
        let _ = store.callees(1);
        let _ = store.unresolved_calls(1);
        let _ = store.cochange_scan();
        let _ = store.cochange_head();
        let _ = store.cochange_of("src/a.rs", 0.0, 5);
        let _ = store.cochange_partners_for_files(&["src/a.rs".to_owned()], 0.0, 5);
        let _ = store.canonical_dump();
    }
}
#[test]
fn every_writer_answers_when_a_table_is_missing() {
    for table in TABLES {
        let (_db, mut store) = store_missing(&format!("write-{table}"), table);
        let language = RustLanguage::new().expect("grammar");
        // Every writer answers a value or a store error, never a panic.
        let _ = store.put_source("src/a.rs", ALPHA, 1, &language);
        let _ = store.remove_file("src/b.rs");
        let _ = store.write_crate_graph(&[], &[], &[], &[]);
        let _ = store.replace_edges(&[]);
        let _ = store.write_links(&[]);
        let _ = store.write_method_owners(&[(1, 1)]);
        let _ = store.reset_symbol_test_flags("src/a.rs", &[true, true]);
        let _ = store.write_rank(2, &[], &[]);
        let _ = store.write_cochange(&crate::CochangeScan::default());
        let _ = store.set_workspace_fingerprint(&[0u8; 32], 0);
        let _ = store.set_extractor_version(11);
    }
}

/// A SQLite file that is not one of ours: a table, no stamp.
fn foreign_database(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).expect("raw sqlite");
    conn.execute_batch("CREATE TABLE not_ours(x TEXT);")
        .expect("table");
}
#[test]
fn open_and_index_paths_refuse_what_they_cannot_read() {
    let db = TempDb::new("defects-open");
    assert_outside_workspace(db.path());
    let not_ours = db.path().with_extension("foreign.db");
    foreign_database(&not_ours);
    // A database with no stamp is not ours, and the read-only open says
    // so rather than writing one.
    assert!(matches!(
        Store::open_read_only(&not_ours),
        Err(crate::StoreError::ForeignDatabase)
    ));
    // A `meta` table of another shape answers through the same read-only
    // path, as a store error rather than a panic.
    let malformed = db.path().with_extension("malformed.db");
    let conn = rusqlite::Connection::open(&malformed).expect("raw sqlite");
    conn.execute_batch("CREATE TABLE meta(x TEXT);")
        .expect("table");
    drop(conn);
    assert!(Store::open_read_only(&malformed).is_err());
    // A read-only open of a database that does not exist answers, and
    // creates nothing.
    let absent = db.path().with_extension("absent.db");
    assert!(Store::open_read_only(&absent).is_err());
    assert!(!absent.exists());
    // A data directory whose name needs escaping still opens read-only:
    // the URI percent-encodes it instead of failing the open.
    let spaced = db.path().with_extension("spaced");
    std::fs::create_dir_all(&spaced).expect("dir");
    let inside = spaced.join("a graph.db");
    drop(Store::open(&inside).expect("write open"));
    let store = Store::open_read_only(&inside).expect("read-only open");
    assert_eq!(store.row_count("files").expect("count"), 0);
    drop(store);
    // The index path names a repo without a file name, and one that is
    // not on disk, without an absolute path in either.
    let root = crate::index_database_path(std::path::Path::new("/"));
    assert!(root.to_string_lossy().ends_with("graph.db"));
    assert!(root.to_string_lossy().contains("/repo-"));
    let missing = crate::index_database_path(std::path::Path::new("/no/such/repo"));
    assert!(missing.to_string_lossy().contains("/repo-"));
}
#[test]
fn a_fingerprint_or_a_target_this_slice_never_writes_is_refused() {
    let db = TempDb::new("defects-graph");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // A stored fingerprint with a character outside the alphabet the
    // writer emits reads as no fingerprint, never as a panic.
    store
        .conn
        .execute(
            "INSERT INTO meta(key, value) VALUES ('workspace_fingerprint', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            ["z".repeat(64)],
        )
        .expect("poison fingerprint");
    assert_eq!(store.workspace_fingerprint().expect("fingerprint"), None);
    // A module naming a target the graph does not carry is refused.
    let package = crate::Package {
        name: "sample".to_owned(),
        version: None,
        manifest_path: "/w/Cargo.toml".to_owned(),
    };
    let target = phantom_fractal_lang::Target {
        id: 1,
        package: "sample".to_owned(),
        name: "sample".to_owned(),
        kind: phantom_fractal_lang::TargetKind::Lib,
        root: "src/a.rs".to_owned(),
    };
    let stray = phantom_fractal_lang::Module {
        id: 2,
        target: 99,
        parent: None,
        path: "crate::stray".to_owned(),
        file: "src/a.rs".to_owned(),
        scope: String::new(),
    };
    assert!(matches!(
        store.write_crate_graph(&[package], &[target], &[stray], &[]),
        Err(crate::StoreError::UnknownTarget(99))
    ));
}
