//! Replace and remove rows (G1a) and the persist rows (G1b).

use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace, fixture_for};
use crate::Store;

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

#[test]
fn put_source_replaces_an_edited_file_rows() {
    let db = TempDb::new("replace");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let path = "src/lib.rs";
    store
        .put_source(path, "pub fn alpha() {}\n", 0, &language)
        .expect("first put");
    store
        .put_source(path, "pub fn alpha() {}\nfn bravo() {}\n", 7, &language)
        .expect("second put");
    let output = store.file_paths().expect("paths");
    assert_eq!(output, vec!["src/lib.rs".to_owned()]);
    assert_eq!(
        store.read_items(path).expect("items"),
        language
            .extract("pub fn alpha() {}\nfn bravo() {}\n")
            .expect("extract")
            .items
    );
    assert!(store.match_count("bravo").expect("new") >= 1);
    assert_eq!(store.match_count("missing_fn_zz").expect("old"), 0);
}

#[test]
fn remove_file_drops_every_file_row() {
    let db = TempDb::new("remove");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture_for("scopes.rs");
    store
        .put_source("fixtures/extract/scopes.rs", &source, mtime_ns, &language)
        .expect("put");
    assert_eq!(store.file_paths().expect("paths").len(), 1);
    let removed = store
        .remove_file("fixtures/extract/scopes.rs")
        .expect("remove");
    assert!(removed, "a row existed");
    assert_eq!(store.file_paths().expect("paths"), Vec::<String>::new());
    assert_eq!(store.row_count("files").expect("files"), 0);
    assert_eq!(store.row_count("symbols").expect("symbols"), 0);
    assert_eq!(
        store
            .read_items("fixtures/extract/scopes.rs")
            .expect("items"),
        Vec::new()
    );
    assert_eq!(store.match_count("Ledger").expect("fts"), 0);
    let removed = store
        .remove_file("fixtures/extract/scopes.rs")
        .expect("remove");
    assert!(!removed, "no row remains");
}

#[test]
fn no_edge_outlives_its_symbol() {
    let db = TempDb::new("edges");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture_for("refs.rs");
    store
        .put_source("fixtures/extract/refs.rs", &source, mtime_ns, &language)
        .expect("put");
    write_lib_graph(&mut store, "fixtures/extract/refs.rs");
    let input = store.resolve_input().expect("input");
    let plain = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "plain")
        .expect("plain")
        .id;
    let target = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Ledger")
        .expect("Ledger struct")
        .id;
    let ref_id = input
        .refs
        .iter()
        .find(|reference| reference.text == "crate::helpers::helper")
        .expect("call ref")
        .id;
    store
        .replace_edges(&[phantom_fractal_lang::Edge {
            ref_id,
            source: plain,
            target,
            confidence: 0.95,
            reason: phantom_fractal_lang::Reason::Local,
        }])
        .expect("edges");
    assert_eq!(store.row_count("edges").expect("edges"), 1);
    // An edit-and-reindex of the same path deletes the file's rows and
    // every edge that named them.
    store
        .put_source(
            "fixtures/extract/refs.rs",
            "pub fn plain() {}\n",
            mtime_ns,
            &language,
        )
        .expect("replace");
    assert_eq!(
        store.row_count("edges").expect("edges"),
        0,
        "no edge outlives a replaced symbol"
    );
    // A removed file drops them the same way.
    store
        .put_source("fixtures/extract/refs.rs", &source, mtime_ns, &language)
        .expect("restore");
    let input = store.resolve_input().expect("input");
    let plain = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "plain")
        .expect("plain")
        .id;
    let target = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Ledger")
        .expect("Ledger struct")
        .id;
    let ref_id = input.refs.iter().next_back().expect("a call ref").id;
    store
        .replace_edges(&[phantom_fractal_lang::Edge {
            ref_id,
            source: plain,
            target,
            confidence: 0.95,
            reason: phantom_fractal_lang::Reason::Local,
        }])
        .expect("edges");
    assert_eq!(store.row_count("edges").expect("edges"), 1);
    let removed = store
        .remove_file("fixtures/extract/refs.rs")
        .expect("remove");
    assert!(removed);
    assert_eq!(
        store.row_count("edges").expect("edges"),
        0,
        "no edge outlives a removed symbol"
    );
}

#[test]
fn methods_become_method_symbols() {
    let db = TempDb::new("methods");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture_for("scopes.rs");
    store
        .put_source("fixtures/extract/scopes.rs", &source, mtime_ns, &language)
        .expect("put");
    write_lib_graph(&mut store, "fixtures/extract/scopes.rs");
    // read_items keeps items only, in the W2 order.
    let extract = language.extract(&source).expect("extract");
    assert_eq!(
        store
            .read_items("fixtures/extract/scopes.rs")
            .expect("items"),
        extract.items
    );
    let input = store.resolve_input().expect("input");
    for method in extract.methods {
        let stored = input
            .symbols
            .iter()
            .find(|symbol| {
                symbol.kind == phantom_fractal_lang::SymbolKind::Method
                    && symbol.name == method.name
                    && symbol.owner.as_deref() == Some(method.owner.as_str())
            })
            .unwrap_or_else(|| panic!("method {} stored", method.name));
        assert_eq!(
            stored.owner_path.as_deref(),
            Some(method.owner_path.as_str())
        );
        assert_eq!(
            stored.trait_name.as_deref(),
            method.trait_name.clone().as_deref()
        );
        assert_eq!(stored.scope, method.scope);
    }
    let byte = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "byte")
        .expect("byte");
    assert_eq!(byte.scope, "outer::inner");
    assert_eq!(byte.owner.as_deref(), Some("Ledger"));
}

#[test]
fn call_refs_carry_scope_receiver_and_shadowed() {
    let db = TempDb::new("callrefs");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture_for("refs.rs");
    store
        .put_source("fixtures/extract/refs.rs", &source, mtime_ns, &language)
        .expect("put");
    write_lib_graph(&mut store, "fixtures/extract/refs.rs");
    let input = store.resolve_input().expect("input");
    let shadowed = input
        .refs
        .iter()
        .find(|reference| reference.text == "build")
        .expect("build(1) is stored");
    assert!(shadowed.shadowed);
    assert_eq!(shadowed.kind, phantom_fractal_lang::RefKind::Call);
    input
        .refs
        .iter()
        .find(|reference| reference.kind == phantom_fractal_lang::RefKind::MethodCall)
        .expect("method call stored");
    // The receiver of a method call is stored even though the resolver
    // contract does not carry it.
    let conn = rusqlite::Connection::open(db.path()).expect("sqlite");
    let receivers = [conn
        .query_row(
            "SELECT COUNT(*) FROM refs WHERE kind = 'method_call' AND receiver IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("receiver rows")];
    assert!(receivers[0] >= 1, "a method call's receiver is stored");
    helper_call_matches_reference(&input, &language, &source);
}

fn helper_call_matches_reference(
    input: &phantom_fractal_lang::ResolveInput,
    language: &RustLanguage,
    source: &str,
) {
    let extract = language.extract(source).expect("extract");
    // Compare as sorted multisets: two call sites can share a text. A
    // call's `from` is stored only when its enclosing symbol has a row —
    // a trait default body's method has none — so the original side is
    // compared as "the file has a row for this encloser".
    let items_only = reference_enclosers(&input.symbols);
    let mut stored: Vec<(String, String, String, bool, bool)> = input
        .refs
        .iter()
        .map(|row| {
            (
                row.text.clone(),
                rows_ref_kind(row.kind),
                row.scope.clone(),
                row.shadowed,
                row.from.is_some(),
            )
        })
        .collect();
    let mut original: Vec<(String, String, String, bool, bool)> = extract
        .refs
        .iter()
        .map(|reference| {
            (
                reference.text.clone(),
                rows_ref_kind(reference.kind),
                reference.scope.clone(),
                reference.shadowed,
                reference.from.as_ref().is_some_and(|caller| {
                    items_only.contains(&(caller.name.clone(), caller.owner.clone()))
                }),
            )
        })
        .collect();
    stored.sort();
    original.sort();
    assert_eq!(stored, original);
}

fn reference_enclosers(
    symbols: &[phantom_fractal_lang::Symbol],
) -> std::collections::HashSet<(String, Option<String>)> {
    symbols
        .iter()
        .map(|symbol| (symbol.name.clone(), symbol.owner.clone()))
        .collect()
}

#[test]
fn use_rows_carry_alias_and_glob() {
    let db = TempDb::new("userows");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture_for("imports.rs");
    store
        .put_source("fixtures/extract/imports.rs", &source, mtime_ns, &language)
        .expect("put");
    let input = store.resolve_input().expect("input");
    let extract = language.extract(&source).expect("extract");
    assert_eq!(input.uses.len(), extract.imports.len());
    for import in &extract.imports {
        let stored = input
            .uses
            .iter()
            .find(|row| row.path == import.path)
            .unwrap_or_else(|| panic!("use {} not stored", import.path));
        assert_eq!(stored.alias.as_deref(), import.alias.as_deref());
        assert_eq!(stored.glob, import.glob);
        assert_eq!(stored.file, "fixtures/extract/imports.rs");
    }
}

/// Replaces the three graph tables with one fake package, one lib target
/// over the given file, and one file-level module row — enough for the
/// rows API's tests to run against a graph without the resolver.
fn rows_ref_kind(kind: phantom_fractal_lang::RefKind) -> String {
    let text = format!("{kind:?}");
    text.to_ascii_lowercase()
}

fn write_lib_graph(store: &mut Store, file: &str) {
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
                root: file.to_string(),
            }],
            &[phantom_fractal_lang::Module {
                id: 1,
                target: 1,
                parent: None,
                path: "crate".to_owned(),
                file: file.to_owned(),
                scope: String::new(),
            }],
        )
        .expect("graph");
}
