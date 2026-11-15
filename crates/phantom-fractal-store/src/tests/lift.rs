//! The W13 G1 coverage lift (SPEC-w13 G1, D8): the error surfaces, the
//! stamp and migration paths, the read-only guard, and the scorer
//! reads the bars ask for. Written from the SPEC and the behavior,
//! never from the lines.

use std::fs;

use phantom_fractal_lang_rust::RustLanguage;
use std::error::Error as _;

use rusqlite::Connection;

use super::{TempDb, assert_outside_workspace};
use crate::{SCHEMA_VERSION, Store, StoreError, WriterLock};

/// One raw stamped database, as the legacy fixtures build them.
fn stamped(label: &str, extra_sql: &str) -> TempDb {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    let conn = Connection::open(db.path()).expect("sqlite");
    conn.execute_batch(
        "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
         INSERT INTO meta(key, value) VALUES ('producer', 'phantom-fractal');",
    )
    .expect("meta");
    conn.execute_batch(extra_sql).expect("stamp");
    drop(conn);
    db
}

#[test]
fn store_error_displays_its_own_text_for_every_variant() {
    let cases: Vec<(StoreError, &str)> = vec![
        (
            StoreError::Sqlite(rusqlite::Error::InvalidColumnName("x".to_owned())),
            "sqlite: Invalid column name: x",
        ),
        (StoreError::Io(std::io::Error::other("boom")), "io: boom"),
        (StoreError::MissingProducer, "database has no producer row"),
        (
            StoreError::ForeignProducer,
            "producer is not phantom-fractal",
        ),
        (
            StoreError::SchemaNewer { found: 9 },
            "schema_version 9 is newer than this binary",
        ),
        (
            StoreError::SchemaMismatch {
                schema_version: 2,
                user_version: 1,
            },
            "user_version 1 does not match schema_version 2",
        ),
        (StoreError::UnreadableStamp, "schema stamp is not readable"),
        (
            StoreError::ForeignDatabase,
            "database is not a phantom-fractal index",
        ),
        (StoreError::UnknownTable, "table name is not allowed"),
        (
            StoreError::Extract(phantom_fractal_lang::ExtractError::Parse),
            "extract: parser returned no tree",
        ),
        (
            StoreError::UnknownKind("weird".to_owned()),
            "unknown kind weird",
        ),
        (
            StoreError::UnknownVisibility("secret".to_owned()),
            "unknown visibility secret",
        ),
        (
            StoreError::UnknownReason("because".to_owned()),
            "unknown edge reason because",
        ),
        (
            StoreError::UnknownPackage("pkg".to_owned()),
            "the graph has no package named pkg",
        ),
        (
            StoreError::UnknownTarget(7),
            "the graph has no target with id 7",
        ),
        (
            StoreError::DataDirectory("repos".to_owned(), "is a symlink".to_owned()),
            "repos: is a symlink",
        ),
        (
            StoreError::NotOwnedByUs("repos".to_owned(), "another uid".to_owned()),
            "repos: another uid",
        ),
        (
            StoreError::LockHeld,
            "another writer holds the database lock",
        ),
        (
            StoreError::ReadOnlyUnavailable,
            "read-only open cannot proceed without a sidecar write",
        ),
        (
            StoreError::OwnFlagCount {
                stored: 2,
                flags: 3,
            },
            "3 own test flags do not match the file's 2 symbols",
        ),
    ];
    for (error, text) in cases {
        assert_eq!(error.to_string(), text, "{text}");
    }
    // Only the wrapped causes carry a source.
    assert!(
        StoreError::Sqlite(rusqlite::Error::InvalidColumnName("x".to_owned()))
            .source()
            .is_some()
    );
    assert!(
        StoreError::Io(std::io::Error::other("boom"))
            .source()
            .is_some()
    );
    assert!(
        StoreError::Extract(phantom_fractal_lang::ExtractError::Parse)
            .source()
            .is_some()
    );
    assert!(StoreError::LockHeld.source().is_none());
    assert!(StoreError::UnknownTarget(7).source().is_none());
}

#[test]
fn an_unreadable_stamp_is_refused_in_every_shape() {
    for (sql, why) in [
        ("", "no version row"),
        (
            "INSERT INTO meta(key, value) VALUES ('schema_version', 'abc');
             PRAGMA user_version = 2;",
            "a version that is not a number",
        ),
        (
            "INSERT INTO meta(key, value) VALUES ('schema_version', '0');
             PRAGMA user_version = 0;",
            "a version the binary does not carry",
        ),
    ] {
        let db = stamped("unreadable", sql);
        let opened = Store::open(db.path()).err().expect("refused");
        assert!(matches!(opened, StoreError::UnreadableStamp), "{why}");
    }
}

#[test]
fn a_user_version_that_disagrees_with_the_stamp_is_refused() {
    let db = stamped(
        "mismatch",
        "INSERT INTO meta(key, value) VALUES ('schema_version', '3');
         PRAGMA user_version = 2;",
    );
    let opened = Store::open(db.path()).err().expect("refused");
    assert!(matches!(
        opened,
        StoreError::SchemaMismatch {
            schema_version: 3,
            user_version: 2,
        }
    ));
}

#[test]
fn a_version_one_database_migrates_through_two_to_three() {
    assert_eq!(SCHEMA_VERSION, 3);
    let db = TempDb::new("v1");
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "ALTER TABLE symbols DROP COLUMN scope;
             ALTER TABLE symbols DROP COLUMN owner;
             ALTER TABLE symbols DROP COLUMN owner_path;
             ALTER TABLE symbols DROP COLUMN trait_name;
             ALTER TABLE modules DROP COLUMN scope;
             ALTER TABLE refs DROP COLUMN scope;
             ALTER TABLE refs DROP COLUMN receiver;
             ALTER TABLE refs DROP COLUMN shadowed;
             ALTER TABLE refs DROP COLUMN alias;
             ALTER TABLE refs DROP COLUMN glob;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';
             PRAGMA user_version = 1;",
        )
        .expect("downgrade to version 1");
    }
    let before = file_bytes_before(db.path());
    let store = Store::open(db.path()).expect("open migrates");
    assert_eq!(store.schema_version().expect("version"), 3);
    assert_eq!(store.user_version().expect("user_version"), 3);
    drop(store);
    // The migration wrote the database, unlike the refusal cases.
    assert_ne!(file_bytes_before(db.path()), before);
}

#[test]
fn a_read_only_open_refuses_a_stamp_it_would_migrate() {
    let db = stamped(
        "v2ro",
        "INSERT INTO meta(key, value) VALUES ('schema_version', '2');
         PRAGMA user_version = 2;",
    );
    let opened = Store::open_read_only(db.path()).err().expect("refused");
    assert!(matches!(
        opened,
        StoreError::SchemaMismatch {
            schema_version: 2,
            user_version: 2,
        }
    ));
}

#[test]
fn the_read_only_guard_answers_the_writer_and_the_sidecars() {
    let db = TempDb::new("guard");
    assert_outside_workspace(db.path());
    Store::open(db.path()).expect("fresh store");
    let store = Store::open_read_only(db.path()).expect("read-only open");
    assert!(store.read_only_still_valid().expect("valid"));

    // A writer taking the lock makes the reads possibly racy.
    let lock = WriterLock::acquire(db.path()).expect("lock");
    assert!(!store.read_only_still_valid().expect("held"));
    drop(lock);
    assert!(store.read_only_still_valid().expect("free again"));

    // A sidecar beside the database does the same.
    fs::write(format!("{}-wal", db.path().display()), b"").expect("wal");
    assert!(!store.read_only_still_valid().expect("sidecar"));
    drop(store);
}

#[test]
fn the_scorer_reads_count_the_macro_argument_and_unresolved_calls() {
    let db = TempDb::new("refs");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let source = "pub fn one() {}\n\
                  pub fn plain_caller() {\n    one();\n}\n\
                  #[test]\nfn macro_caller() {\n    assert_eq!(one(), 0);\n}\n";
    store
        .put_source("refs.rs", source, 1, &RustLanguage::new().expect("grammar"))
        .expect("put");
    // The plain call carries no edge (put_source writes refs, never
    // resolve), and the macro argument is its own ref kind.
    assert_eq!(store.unresolved_named("one").expect("unresolved"), 1);
    assert_eq!(store.macro_args_named("one").expect("macro args"), 1);
}

fn file_bytes_before(path: &std::path::Path) -> Vec<u8> {
    fs::read(path).expect("read database")
}

#[test]
fn the_scorer_reads_answer_their_store_facts() {
    use crate::Package;
    use phantom_fractal_lang::{Id, Link, LinkKind, LinkReason, Module, Node, Target, TargetKind};

    let db = TempDb::new("scoring");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source(
            "src/lib.rs",
            "mod helper;\npub use helper::f;\nuse helper::*;\npub fn caller() {\n    f();\n}\n",
            1,
            &language,
        )
        .expect("put lib");
    store
        .put_source("src/helper.rs", "pub fn f() {}\n", 1, &language)
        .expect("put helper");

    // One package, two targets (the lib and a test target that also
    // claims src/lib.rs), and the modules the declarations walk gives.
    let package = Package {
        name: "fixture".to_owned(),
        version: Some("0.0.0".to_owned()),
        manifest_path: "/w/Cargo.toml".to_owned(),
    };
    let lib_target = Target {
        id: 1,
        package: "fixture".to_owned(),
        name: "fixture".to_owned(),
        kind: TargetKind::Lib,
        root: "src/lib.rs".to_owned(),
    };
    let test_target = Target {
        id: 2,
        package: "fixture".to_owned(),
        name: "lib".to_owned(),
        kind: TargetKind::Test,
        root: "src/lib.rs".to_owned(),
    };
    let root_mod = Module {
        id: 10,
        target: 1,
        parent: None,
        path: "crate".to_owned(),
        file: "src/lib.rs".to_owned(),
        scope: String::new(),
    };
    let helper_mod = Module {
        id: 11,
        target: 1,
        parent: Some(10),
        path: "crate::helper".to_owned(),
        file: "src/helper.rs".to_owned(),
        scope: String::new(),
    };
    let shared_mod = Module {
        id: 12,
        target: 2,
        parent: None,
        path: "crate".to_owned(),
        file: "src/lib.rs".to_owned(),
        scope: String::new(),
    };
    store
        .write_crate_graph(
            &[package],
            &[lib_target, test_target],
            &[root_mod, helper_mod, shared_mod],
            &[],
        )
        .expect("crate graph");

    // The import edge the root package's re-export write gives: the
    // module to the symbol, through the `use` ref.
    let f_id: Id = store
        .symbol_spans()
        .expect("spans")
        .iter()
        .find(|span| span.name == "f" && span.path == "src/helper.rs")
        .expect("f span")
        .id;
    let use_rows = store.use_rows().expect("uses");
    let reexport_ref = use_rows
        .iter()
        .find(|row| !row.glob && row.text == "helper::f")
        .expect("the re-export use ref")
        .id;
    let root_row = store
        .module_rows()
        .expect("modules")
        .iter()
        .find(|(id, row)| {
            row.file_id.is_some() && row.path == "crate" && {
                let _ = id;
                true
            }
        })
        .map(|(id, _)| *id)
        .expect("root module row");
    store
        .write_links(&[Link {
            kind: LinkKind::Import,
            source: Node::Module(root_row),
            target: Node::Symbol(f_id),
            confidence: 0.9,
            reason: LinkReason::UseAlias,
            ref_id: Some(reexport_ref),
        }])
        .expect("links");

    // module_rows: every row with its target, parent, path and file.
    let rows = store.module_rows().expect("module rows");
    assert_eq!(rows.len(), 3);
    let lib_file = rows
        .iter()
        .find(|(_, row)| row.path == "crate" && row.target_id == 1)
        .and_then(|(_, row)| row.file_id)
        .expect("lib file id");
    assert_eq!(
        rows.get(&root_row).and_then(|row| row.file_id),
        Some(lib_file)
    );
    // file_target_counts: src/lib.rs is claimed by two targets.
    let counts = store.file_target_counts().expect("counts");
    assert_eq!(counts.get(&lib_file), Some(&2));
    // use_rows: the two root uses and the caller's file.
    assert_eq!(use_rows.len(), 2);
    assert!(use_rows.iter().any(|row| row.glob));
    // import_targets: the re-export's ref id answers its symbol target.
    let imports = store.import_targets().expect("imports");
    assert_eq!(
        imports.get(&reexport_ref),
        Some(&("symbol".to_owned(), f_id))
    );
    // symbol_modules: f's row is the helper module.
    let helper_row = *rows
        .iter()
        .find(|(_, row)| row.path == "crate::helper")
        .map(|(id, _)| id)
        .expect("helper row");
    let symbol_modules = store.symbol_modules().expect("symbol modules");
    assert_eq!(symbol_modules.get(&f_id), Some(&helper_row));
    // score_call_refs: the caller carries its call ref with its scope.
    let caller_id: Id = store
        .symbol_spans()
        .expect("spans")
        .iter()
        .find(|span| span.name == "caller")
        .expect("caller span")
        .id;
    let refs = store.score_call_refs(caller_id).expect("refs");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].kind, "call");
    assert_eq!(refs[0].text, "f");
    assert_eq!(refs[0].scope, "");

    // The impact reads the scorer leans on: the file's module claim,
    // and the caller's refs with their edge marks.
    assert!(store.file_has_module(lib_file).expect("claimed"));
    let helper_file = rows
        .get(&helper_row)
        .and_then(|row| row.file_id)
        .expect("helper file");
    let impact_refs = store.symbol_call_refs(caller_id).expect("impact refs");
    assert_eq!(impact_refs.len(), 1);
    assert_eq!(impact_refs[0].kind, "call");
    assert!(!impact_refs[0].placed);
    let _ = helper_file;
    let _ = lib_file;
}

/// Every stored-text decoder refuses a value this slice does not write,
/// and the crate-graph writer refuses a target or module that names a
/// row it was not given.
#[test]
fn stored_text_decoders_refuse_values_this_slice_never_writes() {
    use phantom_fractal_lang::{SymbolKind, Target, TargetKind};

    assert!(matches!(
        crate::graph::symbol_kind_from_text("weird"),
        Err(StoreError::UnknownKind(value)) if value == "weird"
    ));
    assert!(matches!(
        crate::graph::target_kind_from_text("weird"),
        Err(StoreError::UnknownKind(value)) if value == "weird"
    ));
    assert!(matches!(
        crate::graph::reason_from_text("because"),
        Err(StoreError::UnknownReason(value)) if value == "because"
    ));
    assert!(matches!(
        crate::rows::ref_kind_from_text("weird"),
        Err(StoreError::UnknownKind(value)) if value == "weird"
    ));
    assert!(matches!(
        crate::rows::visibility_from_text("secret"),
        Err(StoreError::UnknownVisibility(value)) if value == "secret"
    ));
    // The known texts read back, so the refusals are the odd ones out.
    assert_eq!(
        crate::graph::symbol_kind_from_text("function").expect("known kind"),
        SymbolKind::Function
    );
    assert_eq!(
        crate::graph::target_kind_from_text("lib").expect("known target"),
        TargetKind::Lib
    );

    let db = TempDb::new("poison-graph");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // A target that names no package.
    let stray_target = Target {
        id: 1,
        package: "missing".to_owned(),
        name: "missing".to_owned(),
        kind: TargetKind::Lib,
        root: "src/lib.rs".to_owned(),
    };
    assert!(matches!(
        store.write_crate_graph(&[], &[stray_target], &[], &[]),
        Err(StoreError::UnknownPackage(name)) if name == "missing"
    ));
    // A module that names no target.
    let package = crate::Package {
        name: "fixture".to_owned(),
        version: Some("0.0.0".to_owned()),
        manifest_path: "/w/Cargo.toml".to_owned(),
    };
    let module = phantom_fractal_lang::Module {
        id: 5,
        target: 99,
        parent: None,
        path: "crate".to_owned(),
        file: "src/lib.rs".to_owned(),
        scope: String::new(),
    };
    assert!(matches!(
        store.write_crate_graph(&[package], &[], &[module], &[]),
        Err(StoreError::UnknownTarget(99))
    ));
}

/// Poisoned rows answer the decoder errors through the readers: a
/// symbol of an unknown kind, a ref of an unknown kind, and an edge of
/// an unknown reason.
#[test]
fn poisoned_rows_answer_the_decoder_errors_through_the_readers() {
    let db = TempDb::new("poison-rows");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("src/lib.rs", "pub fn one() {}\n", 1, &language)
        .expect("put");
    // A symbol row whose kind text is not one this slice writes.
    store
        .conn
        .execute("UPDATE symbols SET kind = 'weird' WHERE name = 'one'", [])
        .expect("poison symbol");
    assert!(matches!(
        store.read_items("src/lib.rs"),
        Err(StoreError::UnknownKind(value)) if value == "weird"
    ));
    // A ref row with an unknown kind answers the resolve-input read.
    store
        .conn
        .execute(
            "INSERT INTO refs(file_id, kind, text, last_segment, line, col, scope)
             SELECT id, 'weird', 'x', 'x', 1, 1, '' FROM files LIMIT 1",
            [],
        )
        .expect("poison ref");
    assert!(matches!(
        store.resolve_input(),
        Err(StoreError::UnknownKind(value)) if value == "weird"
    ));
    // An edge row with an unknown reason answers the edge read.
    store
        .conn
        .execute("DELETE FROM refs WHERE kind = 'weird'", [])
        .expect("clean ref");
    store
        .conn
        .execute(
            "UPDATE symbols SET kind = 'function' WHERE name = 'one'",
            [],
        )
        .expect("repair symbol");
    let spans = store.symbol_spans().expect("spans");
    let one = spans
        .iter()
        .find(|span| span.name == "one")
        .expect("one")
        .id;
    let call_ref: i64 = store
        .conn
        .query_row(
            "INSERT INTO refs(file_id, from_symbol, kind, text, last_segment, line, col, scope)
             SELECT file_id, id, 'call', 'one', 'one', 1, 1, '' FROM symbols
             WHERE name = 'one' RETURNING id",
            [],
            |row| row.get(0),
        )
        .expect("call ref");
    store
        .conn
        .execute(
            "INSERT INTO edges(kind, src_kind, src_id, dst_kind, dst_id, confidence, reason, ref_id)
             VALUES ('call', 'symbol', ?1, 'symbol', ?1, 0.9, 'because', ?2)",
            rusqlite::params![one, call_ref],
        )
        .expect("poison edge");
    assert!(matches!(
        store.callees(one),
        Err(StoreError::UnknownReason(value)) if value == "because"
    ));
}

/// Every reader answers a real `Sqlite` error when its tables are gone:
/// the arms behind the `?` on each statement, exercised through the
/// public reads.
#[test]
fn readers_answer_the_sqlite_error_when_their_tables_are_gone() {
    let db = TempDb::new("tables-gone");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source(
            "src/lib.rs",
            "pub fn one() {}\npub fn two() {}\n",
            1,
            &language,
        )
        .expect("put");
    store
        .conn
        .execute_batch(
            "DROP TRIGGER IF EXISTS symbols_ai;
             DROP TRIGGER IF EXISTS symbols_ad;
             DROP TRIGGER IF EXISTS symbols_au;
             PRAGMA foreign_keys = OFF;",
        )
        .expect("drop triggers");
    let tables: Vec<String> = store
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name <> 'meta'")
        .expect("list")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("tables");
    for table in &tables {
        store
            .conn
            .execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\";"))
            .expect("drop");
    }

    // Every read that touches a dropped table answers a store error,
    // never a panic.
    assert!(store.symbol_spans().is_err());
    assert!(store.read_items("src/lib.rs").is_err());
    assert!(store.file_paths().is_err());
    assert!(store.file_stats().is_err());
    assert!(store.edges_of_kind("call").is_err());
    assert!(store.resolve_input().is_err());
    assert!(store.symbol_modules().is_err());
    assert!(store.module_rows().is_err());
    assert!(store.file_target_counts().is_err());
    assert!(store.use_rows().is_err());
    assert!(store.import_targets().is_err());
    assert!(store.package_dirs().is_err());
    assert!(store.cochange_scan().is_err());
    assert!(store.rank_input().is_err());
    assert!(store.canonical_dump().is_err());
    assert!(store.symbol_by_stable_key("x").is_err());
    assert!(store.match_count("one").is_err());
    assert!(store.row_count("files").is_err());
    assert!(store.symbol_facts(1).is_err());
    assert!(store.symbol_call_refs(1).is_err());
    assert!(store.macro_args_named("one").is_err());
    assert!(store.unresolved_named("one").is_err());
    assert!(store.file_has_module(1).is_err());
    assert!(store.ref_texts("src/lib.rs").is_err());
    assert!(store.symbol_detail(1).is_err());
    assert!(store.members(1).is_err());
    assert!(store.map_rows().is_err());
    assert!(store.rank_call_edges().is_err());
    assert!(store.rank_import_edges().is_err());
    // The writers answer their errors through the same arms, with one
    // row each so their insert paths run against the broken schema.
    let package = crate::Package {
        name: "gone".to_owned(),
        version: Some("0.0.0".to_owned()),
        manifest_path: "/w/Cargo.toml".to_owned(),
    };
    let target = phantom_fractal_lang::Target {
        id: 1,
        package: "gone".to_owned(),
        name: "gone".to_owned(),
        kind: phantom_fractal_lang::TargetKind::Lib,
        root: "src/lib.rs".to_owned(),
    };
    let module = phantom_fractal_lang::Module {
        id: 2,
        target: 1,
        parent: None,
        path: "crate".to_owned(),
        file: "src/lib.rs".to_owned(),
        scope: String::new(),
    };
    assert!(
        store
            .write_crate_graph(&[package], &[target], &[module], &[])
            .is_err()
    );
    assert!(
        store
            .replace_edges(&[phantom_fractal_lang::Edge {
                ref_id: 1,
                source: 1,
                target: 1,
                confidence: 0.9,
                reason: phantom_fractal_lang::Reason::Local,
            }])
            .is_err()
    );
    assert!(
        store
            .write_links(&[phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Import,
                source: phantom_fractal_lang::Node::Module(1),
                target: phantom_fractal_lang::Node::Symbol(1),
                confidence: 0.9,
                reason: phantom_fractal_lang::LinkReason::Local,
                ref_id: Some(1),
            }])
            .is_err()
    );
    assert!(store.remove_file("src/lib.rs").is_err());
    assert!(
        store
            .put_source("src/lib.rs", "pub fn one() {}\n", 2, &language)
            .is_err()
    );
    // The reads and writers the co-change, rank, map and neighbour
    // families answer through the same arms.
    // A partner read over a path whose file row is gone answers an
    // empty list, never an error.
    assert!(
        store
            .cochange_of("src/lib.rs", 0.0, 5)
            .expect("no file row")
            .is_empty()
    );
    assert!(
        store
            .cochange_partners_for_files(&["src/lib.rs".to_owned()], 0.0, 5)
            .expect("no file row")
            .iter()
            .all(|group| group.partners.is_empty())
    );
    assert!(
        store
            .write_cochange(&crate::CochangeScan::default())
            .is_err()
    );
    assert!(store.stored_rank().is_err());
    assert!(store.files().is_err());
    assert!(store.map_file_paths().is_err());
    assert!(store.write_rank(1, &[], &[]).is_err());
    assert!(store.symbols_matching("one").is_err());
    assert!(store.callers(1).is_err());
    assert!(store.callees(1).is_err());
    assert!(store.unresolved_calls(1).is_err());
    assert!(store.write_method_owners(&[(1, 1)]).is_err());
    assert!(
        store
            .reset_symbol_test_flags("src/lib.rs", &[true])
            .is_err()
    );
    assert!(store.score_call_refs(1).is_err());
    assert!(store.ref_ids_of_kind("call").is_err());
    assert!(store.stored_blake3("src/lib.rs").is_err());
}
