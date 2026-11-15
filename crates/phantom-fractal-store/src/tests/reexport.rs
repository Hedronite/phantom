//! The re-export flag (A1-w15): a `use` row stores its visibility, the
//! store reads which symbols a `pub use` re-exports, a version-3
//! database migrates additively, and a re-index fills the column.

use phantom_fractal_lang::{LinkKind, LinkReason, Module, Node, Target, TargetKind};
use phantom_fractal_lang_rust::RustLanguage;
use rusqlite::Connection;

use super::{TempDb, assert_outside_workspace};
use crate::{SCHEMA_VERSION, Store};

const DEFS: &str = "pub fn alpha() {}\npub fn beta() {}\npub fn gamma() {}\npub fn delta() {}\n";

/// One source per kind of `use`, each naming its own target symbol.
const SOURCES: [(&str, &str); 5] = [
    ("src/a.rs", DEFS),
    ("src/exp.rs", "pub use crate::alpha;\n"),
    ("src/imp.rs", "use crate::beta;\n"),
    ("src/crt.rs", "pub(crate) use crate::gamma;\n"),
    ("src/sup.rs", "pub(super) use crate::delta;\n"),
];

/// `(file of the use, symbol it names)`, in the order the links go in.
const LINKED: [(&str, &str); 4] = [
    ("src/exp.rs", "alpha"),
    ("src/imp.rs", "beta"),
    ("src/crt.rs", "gamma"),
    ("src/sup.rs", "delta"),
];

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

fn put_all(store: &mut Store) {
    let language = language();
    for (path, source) in SOURCES {
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
}

/// One `import` link per use, from the stored module to the symbol the
/// use names, carrying that use's ref row.
fn link_all(store: &mut Store) {
    let input = store.resolve_input().expect("resolve");
    let module = input.modules.first().expect("module").id;
    let links: Vec<_> = LINKED
        .iter()
        .map(|(file, name)| {
            let target = input
                .symbols
                .iter()
                .find(|symbol| symbol.name == *name)
                .expect("symbol")
                .id;
            let use_ref = input
                .uses
                .iter()
                .find(|binding| binding.file == *file)
                .expect("use ref")
                .id;
            phantom_fractal_lang::Link {
                kind: LinkKind::Import,
                source: Node::Module(module),
                target: Node::Symbol(target),
                confidence: 0.95,
                reason: LinkReason::Local,
                ref_id: Some(use_ref),
            }
        })
        .collect();
    store.write_links(&links).expect("links");
}

fn symbol_id(store: &Store, name: &str) -> i64 {
    store
        .resolve_input()
        .expect("resolve")
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect("symbol")
        .id
}

fn visibility_of(store: &Store, file: &str) -> String {
    store
        .conn
        .query_row(
            "SELECT r.visibility FROM refs r JOIN files f ON f.id = r.file_id
             WHERE r.kind = 'use' AND f.path = ?1",
            [file],
            |row| row.get(0),
        )
        .expect("use row")
}

#[test]
fn a_pub_use_and_a_private_use_store_different_rows() {
    let db = TempDb::new("reexport-rows");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    put_all(&mut store);
    assert_eq!(visibility_of(&store, "src/exp.rs"), "public");
    assert_eq!(visibility_of(&store, "src/imp.rs"), "private");
    assert_eq!(visibility_of(&store, "src/crt.rs"), "crate");
    assert_eq!(visibility_of(&store, "src/sup.rs"), "restricted:pub(super)");
    // Every other ref kind keeps the empty default.
    let others: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM refs WHERE kind <> 'use' AND visibility <> ''",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(others, 0);
}

#[test]
fn only_a_pub_use_flags_its_target() {
    let db = TempDb::new("reexport-flag");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    put_all(&mut store);
    link_all(&mut store);
    let alpha = symbol_id(&store, "alpha");
    assert_eq!(
        store
            .use_reexports()
            .expect("set")
            .into_iter()
            .collect::<Vec<_>>(),
        vec![alpha]
    );
    assert!(store.is_reexported(alpha).expect("alpha"));
    for name in ["beta", "gamma", "delta"] {
        let id = symbol_id(&store, name);
        assert!(!store.is_reexported(id).expect(name), "{name}");
    }
    assert!(!store.is_reexported(i64::MAX).expect("unknown id"));
}

#[test]
fn a_store_with_no_pub_use_flags_nothing() {
    let db = TempDb::new("reexport-none");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert!(store.use_reexports().expect("empty store").is_empty());
    let language = language();
    store
        .put_source("src/a.rs", DEFS, 0, &language)
        .expect("put");
    assert!(store.use_reexports().expect("no uses").is_empty());
}

#[test]
fn a_pub_use_with_no_import_edge_flags_nothing() {
    let db = TempDb::new("reexport-noedge");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    put_all(&mut store);
    // The row says `public`, but no resolved import edge points at a
    // symbol, so there is no target to flag.
    assert!(store.use_reexports().expect("no edges").is_empty());
}

#[test]
fn a_version_three_database_migrates_and_reads_false_until_a_reindex() {
    let db = TempDb::new("reexport-v3");
    assert_outside_workspace(db.path());
    {
        let mut store = Store::open(db.path()).expect("open");
        put_all(&mut store);
        link_all(&mut store);
        assert_eq!(store.use_reexports().expect("before").len(), 1);
    }
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "ALTER TABLE refs DROP COLUMN visibility;
             UPDATE meta SET value = '3' WHERE key = 'schema_version';
             PRAGMA user_version = 3;",
        )
        .expect("downgrade to version 3");
    }
    let mut store = Store::open(db.path()).expect("open migrates");
    assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
    assert_eq!(store.user_version().expect("user_version"), SCHEMA_VERSION);
    // The column is there and every carried-over row reads empty.
    assert_eq!(visibility_of(&store, "src/exp.rs"), "");
    assert_eq!(visibility_of(&store, "src/imp.rs"), "");
    let rows: i64 = store.row_count("refs").expect("refs");
    assert!(rows > 0);
    // The import edges survived and the flag reads false.
    assert_eq!(store.edges_of_kind("import").expect("edges").len(), 4);
    assert!(store.use_reexports().expect("migrated").is_empty());
    assert!(
        !store
            .is_reexported(symbol_id(&store, "alpha"))
            .expect("alpha")
    );
    // A re-index re-extracts the file, stores the visibility, and a
    // re-link makes the flag true again.
    let language = language();
    store
        .put_source("src/exp.rs", "pub use crate::alpha;\n", 0, &language)
        .expect("re-put");
    assert_eq!(visibility_of(&store, "src/exp.rs"), "public");
    let input = store.resolve_input().expect("resolve");
    let module = input.modules.first().expect("module").id;
    let alpha = symbol_id(&store, "alpha");
    let use_ref = input
        .uses
        .iter()
        .find(|binding| binding.file == "src/exp.rs")
        .expect("use ref")
        .id;
    store
        .write_links(&[phantom_fractal_lang::Link {
            kind: LinkKind::Import,
            source: Node::Module(module),
            target: Node::Symbol(alpha),
            confidence: 0.95,
            reason: LinkReason::Local,
            ref_id: Some(use_ref),
        }])
        .expect("re-link");
    assert!(store.is_reexported(alpha).expect("after re-index"));
}

#[test]
fn the_migration_is_one_transaction_and_idempotent() {
    let db = TempDb::new("reexport-idem");
    assert_outside_workspace(db.path());
    {
        let mut store = Store::open(db.path()).expect("open");
        put_all(&mut store);
    }
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(
            "ALTER TABLE refs DROP COLUMN visibility;
             UPDATE meta SET value = '3' WHERE key = 'schema_version';
             PRAGMA user_version = 3;",
        )
        .expect("downgrade to version 3");
    }
    let first = Store::open(db.path()).expect("first open");
    let dump = first.canonical_dump().expect("dump");
    drop(first);
    let second = Store::open(db.path()).expect("second open");
    assert_eq!(dump, second.canonical_dump().expect("dump again"));
    assert_eq!(second.schema_version().expect("schema"), SCHEMA_VERSION);
}

#[test]
fn the_dump_names_the_new_column_last_and_shows_a_pub_use() {
    let db = TempDb::new("reexport-dump");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    put_all(&mut store);
    let dump = store.canonical_dump().expect("dump");
    let cols = dump
        .lines()
        .skip_while(|line| *line != "# table refs")
        .nth(1)
        .expect("refs cols line");
    assert!(cols.ends_with("alias\tglob\tvisibility"), "{cols}");
    assert!(
        dump.lines()
            .any(|line| line.starts_with("src/exp.rs") && line.ends_with("\tpublic")),
        "{dump}"
    );
}

/// The migration steps one open of a database stamped `stamp` runs, with
/// the columns later versions added dropped first.
fn open_steps(label: &str, stamp: i64, drop_sql: &str) -> Vec<&'static str> {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    drop(Store::open(db.path()).expect("create"));
    {
        let conn = Connection::open(db.path()).expect("sqlite");
        conn.execute_batch(&format!(
            "{drop_sql}
             UPDATE meta SET value = '{stamp}' WHERE key = 'schema_version';
             PRAGMA user_version = {stamp};"
        ))
        .expect("downgrade");
    }
    crate::open_trace::clear();
    let store = Store::open(db.path()).expect("open");
    assert_eq!(store.schema_version().expect("schema"), SCHEMA_VERSION);
    drop(store);
    crate::open_trace::steps()
        .into_iter()
        .filter(|step| step.starts_with("migrate_"))
        .collect()
}

const DROP_V4: &str = "ALTER TABLE refs DROP COLUMN visibility;";
const DROP_V3_AND_V4: &str = "ALTER TABLE symbols DROP COLUMN scope;
     ALTER TABLE symbols DROP COLUMN owner;
     ALTER TABLE symbols DROP COLUMN owner_path;
     ALTER TABLE symbols DROP COLUMN trait_name;
     ALTER TABLE modules DROP COLUMN scope;
     ALTER TABLE refs DROP COLUMN scope;
     ALTER TABLE refs DROP COLUMN receiver;
     ALTER TABLE refs DROP COLUMN shadowed;
     ALTER TABLE refs DROP COLUMN alias;
     ALTER TABLE refs DROP COLUMN glob;
     ALTER TABLE refs DROP COLUMN visibility;";

#[test]
fn an_open_runs_only_the_migrations_the_stamp_needs() {
    // A current database migrates nothing.
    assert!(open_steps("steps-4", 4, "").is_empty());
    // A version-3 database takes the one new step and no older one.
    assert_eq!(open_steps("steps-3", 3, DROP_V4), vec!["migrate_v3_to_v4"]);
    // A version-2 database skips the first step and takes the other two.
    assert_eq!(
        open_steps("steps-2", 2, DROP_V3_AND_V4),
        vec!["migrate_v2_to_v3", "migrate_v3_to_v4"]
    );
}
