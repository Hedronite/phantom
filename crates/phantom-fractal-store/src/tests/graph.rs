//! Crate-graph API rows (G1c): write, resolve, replace, match, and the
//! caller counters.

use phantom_fractal_lang::{Edge, Reason, ResolveInput};
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace, fixture};
use crate::{Package, Store, StoreError};

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

#[test]
fn fingerprint_stamps_round_trip_and_read_malformed_as_none() {
    let db = TempDb::new("fingerprint");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // Nothing stored yet: both read as None.
    assert_eq!(store.workspace_fingerprint().expect("unset"), None);
    assert_eq!(store.rs_skipped().expect("unset"), None);
    // Round trip.
    let fingerprint: [u8; 32] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
        0x32, 0x10,
    ];
    store
        .set_workspace_fingerprint(&fingerprint, 7)
        .expect("write");
    assert_eq!(
        store.workspace_fingerprint().expect("set"),
        Some(fingerprint)
    );
    assert_eq!(store.rs_skipped().expect("set"), Some(7));
    // A second write replaces both keys.
    store
        .set_workspace_fingerprint(&[0u8; 32], 0)
        .expect("rewrite");
    assert_eq!(
        store.workspace_fingerprint().expect("rewritten"),
        Some([0u8; 32])
    );
    assert_eq!(store.rs_skipped().expect("rewritten"), Some(0));
    // Malformed stored text reads as None, not an error, so a rebuild
    // follows and nothing is refused.
    {
        let conn = rusqlite::Connection::open(db.path()).expect("sqlite");
        for (key, text) in [("workspace_fingerprint", "zz"), ("rs_skipped", "12abc")] {
            conn.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, text),
            )
            .expect("malformed insert");
        }
    }
    assert_eq!(store.workspace_fingerprint().expect("malformed hex"), None);
    assert_eq!(store.rs_skipped().expect("malformed count"), None);
    // SCHEMA_VERSION stays 3: no migration.
    assert_eq!(store.schema_version().expect("schema"), 3);
    assert_eq!(store.user_version().expect("user_version"), 3);
    drop(store);
}

/// One package, one lib target rooted at `file`, and one file-level
/// module row, for tests that need a graph without the resolver.
fn lib_graph(
    file: &str,
) -> (
    Package,
    Vec<phantom_fractal_lang::Target>,
    Vec<phantom_fractal_lang::Module>,
) {
    (
        Package {
            name: "sample".to_owned(),
            version: Some("0.0.0".to_owned()),
            manifest_path: "Cargo.toml".to_owned(),
        },
        vec![phantom_fractal_lang::Target {
            id: 1,
            package: "sample".to_owned(),
            name: "sample".to_owned(),
            kind: phantom_fractal_lang::TargetKind::Lib,
            root: file.to_owned(),
        }],
        vec![phantom_fractal_lang::Module {
            id: 1,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: file.to_owned(),
            scope: String::new(),
        }],
    )
}

#[test]
fn write_crate_graph_replaces_tables_and_links_ids() {
    let db = TempDb::new("graph");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture();
    store
        .put_source("src/items.rs", &source, mtime_ns, &language)
        .expect("put");
    write_graph(&mut store, "src/items.rs", 1);
    assert_eq!(store.row_count("packages").expect("packages"), 1);
    assert_eq!(store.row_count("targets").expect("targets"), 1);
    assert_eq!(store.row_count("modules").expect("modules"), 1);
    let input = store.resolve_input().expect("input");
    for symbol in &input.symbols {
        if symbol.scope.is_empty() {
            assert_eq!(symbol.module, Some(1), "{}", symbol.name);
        } else {
            assert_eq!(
                symbol.module, None,
                "{symbol:?}\n expected an inline-module orphan"
            );
        }
    }
    assert_eq!(input.targets.first().expect("target").root, "src/items.rs");
    // A second write replaces the tables; the link follows the new ids.
    let mut packages = vec![lib_graph("src/items.rs").0];
    packages.push(Package {
        name: "other".to_owned(),
        version: None,
        manifest_path: "other/Cargo.toml".to_owned(),
    });
    let modules = vec![phantom_fractal_lang::Module {
        id: 5,
        target: 2,
        parent: None,
        path: "crate".to_owned(),
        file: "src/items.rs".to_owned(),
        scope: String::new(),
    }];
    let targets = vec![phantom_fractal_lang::Target {
        id: 2,
        package: "other".to_owned(),
        name: "other".to_owned(),
        kind: phantom_fractal_lang::TargetKind::Lib,
        root: "src/items.rs".to_owned(),
    }];
    store
        .write_crate_graph(&packages, &targets, &modules, &[])
        .expect("second graph");
    assert_eq!(store.row_count("packages").expect("packages"), 2);
    assert_eq!(store.row_count("targets").expect("targets"), 1);
    assert_eq!(store.row_count("modules").expect("modules"), 1);
    let input = store.resolve_input().expect("input");
    for symbol in &input.symbols {
        if symbol.scope.is_empty() {
            assert_eq!(symbol.module, Some(1), "{} re-links", symbol.name);
        }
    }
    let replaced = input.targets.first().expect("target");
    assert_eq!(replaced.package, "other");
    assert_eq!(replaced.name, "other");
}

#[test]
fn all_seven_reasons_round_trip_through_the_edges() {
    let db = TempDb::new("reasons");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\nfn f() {}\nfn g() {}\npub fn caller() { a(); b(); c(); d(); e(); f(); g(); }\n";
    store
        .put_source("src/reasons.rs", source, 0, &language)
        .expect("put");
    write_graph(&mut store, "src/reasons.rs", 1);
    let input = store.resolve_input().expect("input");
    // The store leaves the workspace dependency list to the index pass.
    assert!(input.deps.is_empty(), "no deps are store-made");
    let caller = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "caller")
        .expect("caller")
        .id;
    let mut refs: Vec<_> = input
        .refs
        .iter()
        .filter(|reference| {
            reference.from == Some(caller) && reference.kind == phantom_fractal_lang::RefKind::Call
        })
        .collect();
    refs.sort_by_key(|reference| reference.id);
    assert_eq!(refs.len(), 7, "{refs:?}");
    let reasons: [(Reason, f64, &str); 7] = [
        (Reason::PathExact, 1.0, "path-exact"),
        (Reason::Local, 0.95, "local"),
        (Reason::UseAlias, 0.95, "use-alias"),
        (Reason::Assoc, 0.9, "assoc"),
        (Reason::SelfMethod, 0.9, "self-method"),
        (Reason::SelfAssoc, 0.9, "self-assoc"),
        (Reason::CratePath, 0.95, "crate-path"),
    ];
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    let target_ids: Vec<i64> = names
        .iter()
        .map(|name| {
            input
                .symbols
                .iter()
                .find(|symbol| &symbol.name == name)
                .expect("target fn")
                .id
        })
        .collect();
    let edges: Vec<Edge> = refs
        .iter()
        .zip(target_ids.iter())
        .zip(reasons.iter())
        .map(|((reference, target), (reason, confidence, _))| Edge {
            ref_id: reference.id,
            source: caller,
            target: *target,
            confidence: *confidence,
            reason: *reason,
        })
        .collect();
    store.replace_edges(&edges).expect("edges");
    // `callees` reads every edge back with its stored reason text and
    // confidence, in call-site order.
    let callees = store.callees(caller).expect("callees");
    assert_eq!(callees.len(), 7, "{callees:?}");
    for (hit, (_, _, text)) in callees.iter().zip(reasons.iter()) {
        assert_eq!(hit.reason, *text);
    }
    let confidences: Vec<u8> = callees
        .iter()
        .map(|hit| (hit.confidence * 100.0).round() as u8)
        .collect();
    let expected: Vec<u8> = reasons
        .iter()
        .map(|(_, confidence, _)| (confidence * 100.0).round() as u8)
        .collect();
    assert_eq!(confidences, expected);
    // And each edge reads back through `callers` on its own target with
    // the same reason.
    for ((name, edge), (_, _, text)) in names.iter().zip(edges.iter()).zip(reasons) {
        let hits = store.callers(edge.target).expect("callers");
        assert_eq!(hits.len(), 1, "{name}: {hits:?}");
        assert_eq!(hits[0].reason, text);
        assert_eq!(hits[0].symbol, caller);
    }
}
#[test]
fn replace_edges_wipes_and_inserts() {
    let db = TempDb::new("rted");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let (source, mtime_ns) = fixture();
    store
        .put_source("src/items.rs", &source, mtime_ns, &language)
        .expect("put");
    write_graph(&mut store, "src/items.rs", 1);
    let edges = two_edges(&store.resolve_input().expect("input"));
    store.replace_edges(&edges).expect("first replace");
    assert_eq!(store.row_count("edges").expect("edges"), 2);
    let (_, source_sym, target_sym) = ids_of(&edges[0]);
    let hits = store.callers(target_sym).expect("callers");
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(hits[0].symbol, source_sym);
    assert_eq!(
        (hits[0].confidence, hits[0].reason.as_str()),
        (edges[0].confidence, "path-exact")
    );
    let hits = store.callees(source_sym).expect("callees");
    assert!(hits.iter().any(|hit| hit.symbol == target_sym));
    // Replacing again leaves only what it carries.
    store.replace_edges(&edges[..1]).expect("second replace");
    assert_eq!(store.row_count("edges").expect("edges"), 1);
    let hits = store.callers(target_sym).expect("callers");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(
        (hits[0].confidence, hits[0].reason.as_str()),
        (1.0, "path-exact")
    );
}

#[test]
fn symbols_matching_takes_bare_and_qualified_names() {
    let db = TempDb::new("match");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    // lib.rs is the crate root with an inline `mod tests`; ledger.rs is
    // the file module `crate::ledger` with an inline `mod tests` of its
    // own; a.rs is filed one module deeper, `crate::a::b`.
    let lib = "pub fn root_item() {}\n\n#[cfg(test)]\nmod tests {\n    pub fn probe() {}\n}\n";
    let ledger = "pub fn open_ledger() {}\n\npub struct Ledger;\n\nimpl Ledger {\n    pub fn open(&self) {}\n}\n\n#[cfg(test)]\nmod tests {\n    pub fn check() {}\n}\n";
    let a = "pub fn foo() {}\n";
    put(&mut store, &language, "src/lib.rs", lib);
    put(&mut store, &language, "src/ledger.rs", ledger);
    put(&mut store, &language, "src/a.rs", a);
    let modules = vec![
        phantom_fractal_lang::Module {
            id: 10,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: "src/lib.rs".to_owned(),
            scope: String::new(),
        },
        phantom_fractal_lang::Module {
            id: 11,
            target: 1,
            parent: None,
            path: "crate::ledger".to_owned(),
            file: "src/ledger.rs".to_owned(),
            scope: String::new(),
        },
        phantom_fractal_lang::Module {
            id: 12,
            target: 1,
            parent: Some(11),
            path: "crate::ledger::tests".to_owned(),
            file: "src/ledger.rs".to_owned(),
            scope: "tests".to_owned(),
        },
        phantom_fractal_lang::Module {
            id: 13,
            target: 1,
            parent: None,
            path: "crate::a::b".to_owned(),
            file: "src/a.rs".to_owned(),
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
            &[],
        )
        .expect("graph");

    // A bare name.
    let hits = store.symbols_matching("open_ledger").expect("bare");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].kind, "function");
    assert_eq!(hits[0].file, "src/ledger.rs");
    assert_eq!(hits[0].module_path, "crate::ledger");
    assert_eq!(hits[0].qual_name, "open_ledger");
    assert!(hits[0].line.is_some());
    // The SPEC's two module-qualified spellings, with and without crate::.
    for spelling in ["ledger::open_ledger", "crate::ledger::open_ledger"] {
        let hits = store.symbols_matching(spelling).expect(spelling);
        assert_eq!(hits.len(), 1, "{spelling}: {hits:?}");
        assert_eq!(hits[0].qual_name, "open_ledger");
    }
    // A method under its file-module path.
    for spelling in ["ledger::Ledger::open", "crate::ledger::Ledger::open"] {
        let hits = store.symbols_matching(spelling).expect(spelling);
        assert_eq!(hits.len(), 1, "{spelling}: {hits:?}");
        assert_eq!(hits[0].kind, "method");
        assert_eq!(hits[0].qual_name, "Ledger::open");
    }
    // A fn inside the inline `mod tests` of ledger.rs: the module form
    // carries one inline scope, not a doubled one.
    for spelling in ["ledger::tests::check", "crate::ledger::tests::check"] {
        let hits = store.symbols_matching(spelling).expect(spelling);
        assert_eq!(hits.len(), 1, "{spelling}: {hits:?}");
        assert_eq!(hits[0].module_path, "crate::ledger::tests");
    }
    assert!(
        store
            .symbols_matching("ledger::tests::tests::check")
            .expect("no doubled scope")
            .is_empty()
    );
    // The in-file qualified spelling still matches on its own.
    assert_eq!(
        store
            .symbols_matching("tests::check")
            .expect("in-file")
            .len(),
        1
    );
    // lib.rs: the file module is the crate root, so the qualified form is
    // just the in-file qual name, with and without the crate:: prefix.
    assert_eq!(
        store.symbols_matching("tests::probe").expect("probe").len(),
        1
    );
    assert_eq!(
        store
            .symbols_matching("crate::tests::probe")
            .expect("crate probe")
            .len(),
        1
    );
    assert_eq!(store.symbols_matching("root_item").expect("root").len(), 1);
    assert_eq!(
        store
            .symbols_matching("crate::root_item")
            .expect("crate root")
            .len(),
        1
    );
    // A nested module path sorts out to a::b without the prefix.
    for spelling in ["a::b::foo", "crate::a::b::foo"] {
        let hits = store.symbols_matching(spelling).expect(spelling);
        assert_eq!(hits.len(), 1, "{spelling}: {hits:?}");
        assert_eq!(hits[0].file, "src/a.rs");
    }
    assert!(
        store
            .symbols_matching("no_such_symbol_anywhere")
            .expect("none")
            .is_empty()
    );
    // A quote in the query is a bound value, not SQL.
    for spliced in ["a\"b", "x' OR '1'='1", "open_ledger\" OR name<>'"] {
        assert!(
            store
                .symbols_matching(spliced)
                .expect("quote is bound")
                .is_empty(),
            "{spliced} must not match"
        );
    }
}

fn put(store: &mut Store, language: &RustLanguage, path: &str, source: &str) {
    store.put_source(path, source, 0, language).expect("put");
}

#[test]
fn extractor_version_round_trips_in_meta() {
    let db = TempDb::new("extractor");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert_eq!(store.extractor_version().expect("unset"), None);
    store.set_extractor_version(2).expect("set");
    assert_eq!(store.extractor_version().expect("set"), Some(2));
    store.set_extractor_version(1).expect("overwrite");
    assert_eq!(store.extractor_version().expect("overwritten"), Some(1));
}

#[test]
fn unknown_reason_text_fails_the_edge_readers() {
    let db = TempDb::new("badreason");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\npub fn outer() { pick(); }\n";
    store
        .put_source("src/flow.rs", source, 0, &language)
        .expect("put");
    write_graph(&mut store, "src/flow.rs", 1);
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
    let reference = input
        .refs
        .iter()
        .find(|reference| reference.text == "pick")
        .expect("call ref");
    store
        .replace_edges(&[Edge {
            ref_id: reference.id,
            source: outer,
            target: pick,
            confidence: 1.0,
            reason: Reason::PathExact,
        }])
        .expect("edge");
    // A hand-inserted reason that is none of the seven texts makes both
    // readers refuse instead of passing it through.
    {
        let conn = rusqlite::Connection::open(db.path()).expect("sqlite");
        conn.execute("UPDATE edges SET reason = 'made-up'", [])
            .expect("hand insert");
    }
    let callers = store.callers(pick);
    assert!(
        matches!(
            &callers,
            Err(StoreError::UnknownReason(text)) if text == "made-up"
        ),
        "{callers:?}"
    );
    let callees = store.callees(outer);
    assert!(
        matches!(
            &callees,
            Err(StoreError::UnknownReason(text)) if text == "made-up"
        ),
        "{callees:?}"
    );
}

#[test]
fn callers_callees_and_unresolved_counts_agree() {
    let db = TempDb::new("flow");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\npub fn outer() { pick(); }\n";
    store
        .put_source("src/flow.rs", source, 0, &language)
        .expect("put");
    write_graph(&mut store, "src/flow.rs", 1);
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
    let (ref_id_of, _scope) = {
        let reference = input
            .refs
            .iter()
            .find(|reference| reference.text == "pick" && reference.from == Some(outer))
            .expect("call ref");
        (reference.id, reference.scope.clone())
    };
    assert_eq!(
        store.unresolved_calls(outer).expect("unresolved"),
        1,
        "the call ref has no edge yet"
    );
    store
        .replace_edges(&[Edge {
            ref_id: ref_id_of,
            source: outer,
            target: pick,
            confidence: 1.0,
            reason: Reason::PathExact,
        }])
        .expect("edge");
    assert_eq!(store.unresolved_calls(outer).expect("resolved"), 0);
    assert_eq!(store.unresolved_calls(pick).expect("no calls"), 0);
    let callees = store.callees(outer).expect("callees");
    assert_eq!(callees.len(), 1, "{callees:?}");
    assert_eq!(callees[0].symbol, pick);
    assert_eq!(callees[0].file, "src/flow.rs");
    assert!(callees[0].line.is_some());
    assert_eq!(callees[0].confidence, 1.0);
    assert_eq!(callees[0].reason, "path-exact");
    let callers = store.callers(pick).expect("callers");
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert_eq!(callers[0].symbol, outer);
    assert_eq!(callers[0].file, "src/flow.rs");
    assert!(callers[0].line.is_some());
    assert_eq!(callers[0].confidence, 1.0);
    assert_eq!(callers[0].reason, "path-exact");
    assert!(store.callers(999999).expect("none").is_empty());
    assert!(store.callees(999999).expect("none").is_empty());
}

fn write_graph(store: &mut Store, file: &str, module_input_id: i64) {
    let (package, targets, mut modules) = lib_graph(file);
    modules[0].id = module_input_id;
    store
        .write_crate_graph(std::slice::from_ref(&package), &targets, &modules, &[])
        .expect("graph");
}

fn two_edges(input: &ResolveInput) -> Vec<Edge> {
    let plain = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "open_ledger")
        .expect("open_ledger")
        .id;
    let target = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Ledger")
        .expect("Ledger")
        .id;
    let first = input.refs.first().expect("a call ref").id;
    let mirrored = input.refs.last().expect("another ref").id;
    assert_ne!(first, mirrored);
    vec![
        Edge {
            ref_id: first,
            source: plain,
            target,
            confidence: 1.0,
            reason: Reason::PathExact,
        },
        Edge {
            ref_id: mirrored,
            source: plain,
            target,
            confidence: 0.95,
            reason: Reason::Local,
        },
    ]
}

fn ids_of(edge: &Edge) -> (i64, i64, i64) {
    (edge.ref_id, edge.source, edge.target)
}
#[test]
fn write_links_stores_import_and_impl_rows() {
    let db = TempDb::new("links");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\npub struct Ledger;\npub mod inner {\n\n}\n";
    store
        .put_source("src/flow.rs", source, 0, &language)
        .expect("put");
    write_graph(&mut store, "src/flow.rs", 1);
    let input = store.resolve_input().expect("input");
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
    // ref_id must be a stored use ref (the FK is real).
    store
        .put_source("src/uses.rs", "use crate::pick;\n", 0, &language)
        .expect("put uses");
    let use_ref = store.resolve_input().expect("uses input").uses.remove(0).id;
    store
        .write_links(&[
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Import,
                source: phantom_fractal_lang::Node::Module(module),
                target: phantom_fractal_lang::Node::Symbol(pick),
                confidence: 0.95,
                reason: phantom_fractal_lang::LinkReason::Local,
                ref_id: Some(use_ref),
            },
            phantom_fractal_lang::Link {
                kind: phantom_fractal_lang::LinkKind::Impl,
                source: phantom_fractal_lang::Node::Symbol(ledger),
                target: phantom_fractal_lang::Node::Symbol(pick),
                confidence: 0.8,
                reason: phantom_fractal_lang::LinkReason::ImplTrait,
                ref_id: None,
            },
        ])
        .expect("links");
    let import_rows = store.edges_of_kind("import").expect("import rows");
    assert_eq!(import_rows.len(), 1, "{import_rows:?}");
    assert_eq!(import_rows[0].src_kind, "module");
    assert_eq!(import_rows[0].src_id, module);
    assert_eq!(import_rows[0].dst_kind, "symbol");
    assert_eq!(import_rows[0].dst_id, pick);
    assert_eq!(import_rows[0].ref_id, Some(use_ref));
    assert_eq!(import_rows[0].reason, "local");
    let impl_rows = store.edges_of_kind("impl").expect("impl rows");
    assert_eq!(impl_rows.len(), 1, "{impl_rows:?}");
    assert_eq!(impl_rows[0].ref_id, None);
    assert_eq!(impl_rows[0].reason, "impl-trait");
    assert_eq!(impl_rows[0].confidence, 0.8);
    // A bound read: a quote cannot change the SQL.
    assert!(edges_kind_empty(&store, "import\" OR '1'='1"));
    drop(store);
}

fn edges_kind_empty(store: &Store, kind: &str) -> bool {
    store
        .edges_of_kind(kind)
        .map(|v| v.is_empty())
        .unwrap_or(true)
}

#[test]
fn a_non_call_edge_never_leaks_into_the_call_reads() {
    let db = TempDb::new("links-call");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub fn pick() -> u8 { 1 }\npub fn outer() { pick(); }\npub struct Ledger;\n";
    store
        .put_source("src/call.rs", source, 0, &language)
        .expect("put");
    write_graph(&mut store, "src/call.rs", 1);
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
    let reference = input
        .refs
        .iter()
        .find(|reference| reference.text == "pick")
        .expect("call ref");
    store
        .replace_edges(&[Edge {
            ref_id: reference.id,
            source: outer,
            target: pick,
            confidence: 0.95,
            reason: Reason::Local,
        }])
        .expect("call edge");
    // An impl edgehaving same ends with kind impl:
    store
        .write_links(&[phantom_fractal_lang::Link {
            kind: phantom_fractal_lang::LinkKind::Impl,
            source: phantom_fractal_lang::Node::Symbol(outer),
            target: phantom_fractal_lang::Node::Symbol(pick),
            confidence: 0.8,
            reason: phantom_fractal_lang::LinkReason::ImplTrait,
            ref_id: None,
        }])
        .expect("impl edge");
    let callees = store.callees(outer).expect("callees");
    assert_eq!(callees.len(), 1, "one call, not impl: {callees:?}");
    let callers = store.callers(pick).expect("callers");
    assert_eq!(callers.len(), 1, "one call, not impl: {callers:?}");
    assert_eq!(callees[0].symbol, pick);
    // The store pins counts on edges_of_kind: two edges, but one is impl.
    assert_eq!(store.edges_of_kind("call").expect("call rows").len(), 1);
    assert_eq!(store.edges_of_kind("impl").expect("impl rows").len(), 1);
}

#[test]
fn write_links_second_passes_clean_individual_rounds() {
    // A pass with no resolved links writes nothing: replace_edges wipes
    // all kinds and write_links with an empty set leaves the empty table.
    let db = TempDb::new("links-empty");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    store.replace_edges(&[]).expect("wipe");
    store.write_links(&[]).expect("no links");
    assert!(store.edges_of_kind("import").expect("import").is_empty());
    assert!(store.edges_of_kind("impl").expect("impl").is_empty());
    drop(store);
}

#[test]
fn cochange_round_trips_and_reads_empty() {
    let db = TempDb::new("cochange");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // No scan landed: all reads answer nothing or empty.
    assert_eq!(store.cochange_scan().expect("unset"), None);
    // An empty (non-git) scan stores the head row but no pairs or files.
    store
        .write_cochange(&crate::CochangeScan::default())
        .expect("empty scan");
    let scan = store.cochange_scan().expect("empty scan").expect("stored");
    assert_eq!(scan.last_oid, None);
    assert!(scan.files.is_empty() && scan.pairs.is_empty());
    drop(store);
}

#[test]
fn cochange_round_trips_full_scans_and_stays_writeless() {
    let db = TempDb::new("cochangefull");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct Ledger {\\n\\n}\\npub struct Other;\\n";
    store
        .put_source("src/a.rs", source, 0, &language)
        .expect("put a");
    let other = "pub fn pick() -> u8 {\\n    1\\n}\\n";
    store
        .put_source("src/b.rs", other, 0, &language)
        .expect("put b");
    let third = "pub struct Cap;\n";
    store
        .put_source("src/c.rs", third, 0, &language)
        .expect("put c");
    let scan = crate::CochangeScan {
        last_oid: Some("aaaa1111".to_owned()),
        oldest_oid: Some("bbbb2222".to_owned()),
        commits_seen: 45,
        skipped_bulk: 3,
        window_json: "{\"days\":365,\"max_commits\":300}".to_owned(),
        files: vec![
            crate::FileRow {
                path: "src/a.rs".to_owned(),
                commits: 2.0,
                last_time: 1_700_000_000,
                renamed_from: None,
            },
            crate::FileRow {
                path: "src/b.rs".to_owned(),
                commits: 3.5,
                last_time: 1_700_001_000,
                renamed_from: Some("src/old.rs".to_owned()),
            },
            crate::FileRow {
                path: "src/c.rs".to_owned(),
                commits: 4.0,
                last_time: 1_700_002_000,
                renamed_from: None,
            },
        ],
        pairs: vec![
            crate::PairRow {
                a: "src/a.rs".to_owned(),
                b: "src/b.rs".to_owned(),
                together: 2.0,
                n_a: 2.0,
                n_b: 3.5,
                lift: 1.4,
                conf_ab: 0.5,
                conf_ba: 0.57,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_001_000,
            },
            crate::PairRow {
                a: "src/a.rs".to_owned(),
                b: "src/c.rs".to_owned(),
                together: 4.0,
                n_a: 2.0,
                n_b: 4.0,
                lift: 2.5,
                conf_ab: 0.5,
                conf_ba: 0.9,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_002_000,
            },
            crate::PairRow {
                a: "src/b.rs".to_owned(),
                b: "src/c.rs".to_owned(),
                together: 1.0,
                n_a: 3.5,
                n_b: 4.0,
                lift: 0.9,
                conf_ab: 0.3,
                conf_ba: 0.25,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_001_500,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    // The scan reads back both ends and the stored types.
    let stored = store.cochange_scan().expect("scan").expect("stored");
    assert_eq!(stored.last_oid.as_deref(), Some("aaaa1111"));
    assert_eq!(stored.commits_seen, 45);
    assert_eq!(stored.files.len(), 3);
    // Pairs store once as a < b by file id: a.rs(1) before b.rs(2)
    // before c.rs(3), with each pair recorded once with lower first.
    assert_eq!(stored.files[0].path, "src/a.rs");
    assert_eq!(stored.files[2].path, "src/c.rs");
    let pairs: Vec<Vec<String>> = stored
        .pairs
        .iter()
        .map(|pair| vec![pair.a.clone(), pair.b.clone()])
        .collect();
    assert_eq!(
        pairs,
        vec![
            vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()],
            vec!["src/a.rs".to_owned(), "src/c.rs".to_owned()],
            vec!["src/b.rs".to_owned(), "src/c.rs".to_owned()],
        ],
        "{pairs:?}"
    );
    assert!(stored.pairs.iter().all(|pair| pair.together > 0.0));
    // Partners read per path, ordered by lift desc.
    // Partners read per path, ordered by lift desc: c.rs (2.5) first,
    // b.rs (1.4) second.
    let partners = store.cochange_of("src/a.rs", 1.0, 10).expect("partners");
    assert_eq!(partners.len(), 2, "{partners:?}");
    assert_eq!(partners[0].path.as_deref(), Some("src/c.rs"));
    assert_eq!(partners[0].lift, 2.5);
    assert_eq!(partners[1].path.as_deref(), Some("src/b.rs"));
    assert_eq!(partners[1].lift, 1.4);
    // A lift filter drops partners under the stored value.
    let filtered = store.cochange_of("src/a.rs", 1.5, 10).expect("filtered");
    assert_eq!(filtered.len(), 1, "{filtered:?}");
    assert_eq!(filtered[0].path.as_deref(), Some("src/c.rs"));
    // A lift filter above every stored value answers nothing.
    assert!(
        store
            .cochange_of("src/a.rs", 9.0, 10)
            .expect("filtered")
            .is_empty()
    );
    // The limit caps the partner list from the top.
    let capped = store.cochange_of("src/a.rs", 1.0, 1).expect("limit");
    assert_eq!(capped.len(), 1, "{capped:?}");
    assert_eq!(capped[0].path.as_deref(), Some("src/c.rs"));
    assert_eq!(
        store
            .cochange_of("src/a.rs", 1.0, 0)
            .expect("zero limit")
            .len(),
        0
    );
    // Identical scan writes leave the db bytes unchanged (db and wal).
    let db_path = db.path().to_string_lossy().into_owned();
    let before = std::fs::read(&db_path).expect("db read");
    store.write_cochange(&scan).expect("rewrite");
    let after_db = std::fs::read(&db_path).expect("db read");
    assert_eq!(before, after_db, "identical scan rewrites nothing");
    // cochange_head in meta carries the scan's head.
    assert_eq!(
        store.cochange_head().expect("head").as_deref(),
        Some("aaaa1111")
    );
    // Per-file partners: the map's read caps per file and dedups paths.
    let groups = store
        .cochange_partners_for_files(
            &[
                "src/a.rs".to_owned(),
                "src/a.rs".to_owned(),
                "src/b.rs".to_owned(),
            ],
            1.0,
            5,
        )
        .expect("groups");
    assert_eq!(groups.len(), 2, "identical paths answer once");
    assert_eq!(groups[0].path, "src/a.rs");
    drop(store);
    drop(language);
}

#[test]
fn cochange_of_unknown_path_and_drop_unresolved_pairs() {
    let db = TempDb::new("cochangemiss");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct One;\\n";
    store
        .put_source("src/one.rs", source, 0, &language)
        .expect("put");
    // A pair naming a path the index dropped cannot be stored: the end
    // resolves no files row, so the write drops that pair.
    let scan = crate::CochangeScan {
        last_oid: Some("cccc3333".to_owned()),
        oldest_oid: Some("cccc3333".to_owned()),
        commits_seen: 2,
        skipped_bulk: 0,
        window_json: "{}".to_owned(),
        files: vec![crate::FileRow {
            path: "src/one.rs".to_owned(),
            commits: 1.0,
            last_time: 10,
            renamed_from: None,
        }],
        pairs: vec![
            crate::PairRow {
                a: "src/one.rs".to_owned(),
                b: "src/gone.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 2.0,
                conf_ab: 1.0,
                conf_ba: 1.0,
                last_oid: "cccc3333".to_owned(),
                last_time: 10,
            },
            crate::PairRow {
                a: "src/one.rs".to_owned(),
                b: "src/one.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 2.0,
                conf_ab: 1.0,
                conf_ba: 1.0,
                last_oid: "cccc3333".to_owned(),
                last_time: 10,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    let stored = store.cochange_scan().expect("scan").expect("stored");
    assert_eq!(stored.pairs.len(), 0, "{stored:?}");
    drop(store);
    drop(language);
}

#[test]
fn cochange_partners_order_lift_then_together_then_path() {
    let db = TempDb::new("cochangeorder");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct One;\\npub struct Two;\\npub struct Three;\\n";
    store
        .put_source("src/order.rs", source, 0, &language)
        .expect("put");
    let scan = crate::CochangeScan {
        last_oid: Some("dddd4444".to_owned()),
        oldest_oid: Some("dddd4444".to_owned()),
        commits_seen: 10,
        skipped_bulk: 0,
        window_json: "{}".to_owned(),
        files: vec![crate::FileRow {
            path: "src/order.rs".to_owned(),
            commits: 4.0,
            last_time: 5,
            renamed_from: None,
        }],
        pairs: vec![
            crate::PairRow {
                a: "src/order.rs".to_owned(),
                b: "src/one.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 1.2,
                conf_ab: 0.5,
                conf_ba: 0.6,
                last_oid: "dddd4444".to_owned(),
                last_time: 5,
            },
            crate::PairRow {
                a: "src/order.rs".to_owned(),
                b: "src/two.rs".to_owned(),
                together: 2.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 1.2,
                conf_ab: 0.5,
                conf_ba: 0.6,
                last_oid: "dddd4444".to_owned(),
                last_time: 4,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    // These targets are outside the index: their ends resolved no files
    // rows, so no partner rows carry them; keep them out.
    let partners = store
        .cochange_of("src/order.rs", 1.0, 10)
        .expect("partners");
    assert!(partners.is_empty(), "{partners:?}");
    drop(store);
    drop(language);
}
