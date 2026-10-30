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

#[test]
fn trait_default_body_call_gets_no_from_symbol() {
    let db = TempDb::new("traitbody");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub trait Greet {\n    fn greet(&self) {\n        helper();\n    }\n}\n\nimpl Greet for Ledger {\n    fn greet(&self) {\n        wave();\n    }\n}\n\npub struct Ledger;\n\npub fn helper() {}\n\npub fn wave() {}\n\n";
    store
        .put_source("src/greet.rs", source, 0, &language)
        .expect("put");
    let input = store.resolve_input().expect("input");
    // The impl method owns its own call: the from row is the method
    // `greet` on the impl, not the item row of anything co-named.
    let greet_method = input
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == "greet" && symbol.kind == phantom_fractal_lang::SymbolKind::Method
        })
        .expect("impl method row");
    let wave_call = input
        .refs
        .iter()
        .find(|reference| reference.text == "wave")
        .expect("wave stored");
    assert_eq!(
        wave_call.from,
        Some(greet_method.id),
        "impl body encloses wave"
    );
    // The trait default body has no symbol row, so its call borrows
    // nothing — never the impl method.
    let helper_call = input
        .refs
        .iter()
        .find(|reference| reference.text == "helper")
        .expect("helper stored");
    assert_eq!(helper_call.from, None, "trait body borrows nothing");
    drop(store);
}

#[test]
fn nested_inner_fn_call_gets_no_from_symbol() {
    let db = TempDb::new("nested");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "fn inner() { 0 }\n\npub fn outer() {\n    fn inner() { helper(); }\n    helper();\n}\n\npub fn helper() {}\n\n";
    store
        .put_source("src/nested.rs", source, 0, &language)
        .expect("put");
    let input = store.resolve_input().expect("input");
    let outer_id = row_id(&input, "outer");
    let calls: Vec<_> = input
        .refs
        .iter()
        .filter(|reference| reference.text == "helper")
        .collect();
    assert_eq!(calls.len(), 2, "one call per inner fn");
    let outer_starts_nested = calls
        .iter()
        .any(|reference| reference.from == Some(outer_id));
    assert!(outer_starts_nested);
    // The nested inner has no symbol row of its own.
    assert!(calls.iter().any(|reference| reference.from.is_none()));
    drop(store);
}

#[test]
fn cfg_twins_match_by_start_line() {
    let db = TempDb::new("twins");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "#[cfg(unix)]\nfn twin() { helper(); }\n\n#[cfg(not(unix))]\nfn twin() { helper(); }\n\npub fn helper() {}\n\n";
    store
        .put_source("src/twins.rs", source, 0, &language)
        .expect("put");
    let input = store.resolve_input().expect("input");
    let twins: Vec<_> = input
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "twin")
        .collect();
    assert_eq!(twins.len(), 2, "both twins are rows");
    let calls: Vec<_> = input
        .refs
        .iter()
        .filter(|reference| reference.text == "helper")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].from, calls[1].from, "each twin owns its own call");
    assert_eq!(calls[0].from, Some(twins[0].id));
    assert_eq!(calls[1].from, Some(twins[1].id));
    drop(store);
}

fn row_id(input: &phantom_fractal_lang::ResolveInput, name: &str) -> i64 {
    input
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .expect(name)
        .id
}

fn rows_ref_kind(kind: phantom_fractal_lang::RefKind) -> String {
    let text = format!("{kind:?}");
    text.to_ascii_lowercase()
}

#[test]
fn macro_arg_round_trips_and_leaves_the_counts_alone() {
    let db = TempDb::new("macroarg");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn outer() {\n    f();\n    make_it_happen!(f(1));\n}\n\npub fn f(x: u32) -> u32 { x }\n";
    store
        .put_source("src/ma.rs", source, 0, &language)
        .expect("put");
    let input = store.resolve_input().expect("input");
    let args: Vec<_> = input
        .refs
        .iter()
        .filter(|reference| reference.kind == phantom_fractal_lang::RefKind::MacroArg)
        .collect();
    assert_eq!(args.len(), 1, "{args:?}");
    assert_eq!(args[0].text, "f");
    let calls: Vec<_> = input
        .refs
        .iter()
        .filter(|reference| {
            reference.kind == phantom_fractal_lang::RefKind::Call && reference.text == "f"
        })
        .collect();
    assert_eq!(calls.len(), 1, "the macro body does not hide the real call");
    let outer = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "outer")
        .expect("outer")
        .id;
    // `unresolved_calls` counts call, method_call, and macro — the
    // macro invocation and the plain call, not the macro_arg row — and
    // `unresolved_named` only answers call and method_call names.
    assert_eq!(store.unresolved_calls(outer).expect("calls"), 2);
    assert_eq!(
        store
            .unresolved_named("make_it_happen")
            .expect("macro name"),
        0
    );
    assert_eq!(store.macro_args_named("f").expect("macro args"), 1);
    assert_eq!(store.unresolved_named("f").expect("unresolved"), 1);
    drop(store);
}

#[test]
fn is_test_is_written_from_the_extract_and_the_test_modules() {
    let db = TempDb::new("istest");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let attribute = "#[test]\nfn probe() {}\n\n#[tokio::test]\nasync fn so() {}\n\nfn plain() {}\n";
    store
        .put_source("src/attr.rs", attribute, 0, &language)
        .expect("put");
    let inline =
        "fn plain_root() {}\n\n#[cfg(test)]\nmod tests {\n    pub fn inner_probe() {}\n}\n";
    store
        .put_source("src/inline.rs", inline, 0, &language)
        .expect("put");
    store
        .put_source(
            "src/main.rs",
            "#[cfg(test)]\nmod tests;\n\nfn runner() {}\n",
            0,
            &language,
        )
        .expect("put");
    store
        .put_source("src/tests.rs", "pub fn inner() {}\n", 0, &language)
        .expect("put");

    // Attribute tests are flagged by the extract itself, before any graph.
    let probe = symbol_id(&store.resolve_input().expect("input"), "probe");
    let so = symbol_id(&store.resolve_input().expect("input"), "so");
    let plain = symbol_id(&store.resolve_input().expect("input"), "plain");
    let probe_facts = store.symbol_facts(probe).expect("facts").expect("some");
    assert!(probe_facts.is_test);
    let so_facts = store.symbol_facts(so).expect("facts").expect("some");
    assert!(so_facts.is_test);
    let plain_facts = store.symbol_facts(plain).expect("facts").expect("some");
    assert!(!plain_facts.is_test);

    // `test_modules` marks both an inline cfg(test) module and a file
    // module declared with #[cfg(test)] — and nothing outside them.
    let modules = vec![
        phantom_fractal_lang::Module {
            id: 20,
            target: 1,
            parent: None,
            path: "crate::attr".to_owned(),
            file: "src/attr.rs".to_owned(),
            scope: String::new(),
        },
        phantom_fractal_lang::Module {
            id: 21,
            target: 1,
            parent: None,
            path: "crate::inline".to_owned(),
            file: "src/inline.rs".to_owned(),
            scope: String::new(),
        },
        phantom_fractal_lang::Module {
            id: 22,
            target: 1,
            parent: Some(21),
            path: "crate::inline::tes".to_owned(),
            file: "src/inline.rs".to_owned(),
            scope: "tests".to_owned(),
        },
        phantom_fractal_lang::Module {
            id: 23,
            target: 1,
            parent: None,
            path: "crate::app".to_owned(),
            file: "src/main.rs".to_owned(),
            scope: String::new(),
        },
        phantom_fractal_lang::Module {
            id: 24,
            target: 1,
            parent: Some(23),
            path: "crate::app::tests".to_owned(),
            file: "src/tests.rs".to_owned(),
            scope: String::new(),
        },
    ];
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
            &modules,
            &[22, 24],
        )
        .expect("graph");

    let input = store.resolve_input().expect("input");
    let inner_probe = symbol_id(&input, "inner_probe");
    let facts = store
        .symbol_facts(inner_probe)
        .expect("facts")
        .expect("some");
    assert!(facts.is_test, "inline test module member");
    assert_eq!(facts.package.as_deref(), Some("sample"));
    assert_eq!(facts.target_kind.as_deref(), Some("lib"));
    let inner = symbol_id(&input, "inner");
    let facts = store.symbol_facts(inner).expect("facts").expect("some");
    assert!(facts.is_test, "file-module test member");
    let plain_root = symbol_id(&input, "plain_root");
    let facts = store
        .symbol_facts(plain_root)
        .expect("facts")
        .expect("some");
    assert!(!facts.is_test, "outside the test modules");
    let runner = symbol_id(&input, "runner");
    let facts = store.symbol_facts(runner).expect("facts").expect("some");
    assert!(!facts.is_test, "outside the test modules");
    drop(store);
}

fn symbol_id(input: &phantom_fractal_lang::ResolveInput, name: &str) -> i64 {
    input
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("symbol {name} not stored"))
        .id
}

/// Replaces the three graph tables with one fake package, one lib target
/// over the given file, and one file-level module row — enough for the
/// rows API's tests to run against a graph without the resolver.
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
            &[],
        )
        .expect("graph");
}

#[test]
fn file_stats_reads_sorted_stat_rows() {
    let db = TempDb::new("filestats");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert!(store.file_stats().expect("empty stats").is_empty());
    let language = language();
    store
        .put_source("src/c.rs", "pub struct Cap;\n", 20, &language)
        .expect("put c");
    store
        .put_source("src/a.rs", "pub fn alpha11() {}\n", 7, &language)
        .expect("put a");
    let stats = store.file_stats().expect("stats");
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].path, "src/a.rs");
    assert_eq!(stats[1].path, "src/c.rs");
    assert_eq!(stats[0].size, "pub fn alpha11() {}\n".len() as i64);
    assert_eq!(stats[0].mtime_ns, 7);
    assert_eq!(
        stats[0].blake3,
        blake3::hash("pub fn alpha11() {}\n".as_bytes())
            .as_bytes()
            .to_vec()
    );
    assert_eq!(stats[1].size, "pub struct Cap;\n".len() as i64);
    assert_eq!(stats[1].mtime_ns, 20);
    assert_eq!(stats[1].blake3.len(), 32);
    drop(store);
    drop(language);
}
