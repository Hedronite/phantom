//! Crate-graph API rows (G1c): write, resolve, replace, match, and the
//! caller counters.

use phantom_fractal_lang::{Edge, Reason, ResolveInput};
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace, fixture};
use crate::{Package, Store, StoreError};

pub(super) fn language() -> RustLanguage {
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
    // SCHEMA_VERSION stays 4: no migration.
    assert_eq!(store.schema_version().expect("schema"), 4);
    assert_eq!(store.user_version().expect("user_version"), 4);
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

pub(super) fn put(store: &mut Store, language: &RustLanguage, path: &str, source: &str) {
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
fn reset_symbol_test_flags_restores_own_flags() {
    use phantom_fractal_lang::Language;

    let db = TempDb::new("ownflags");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "#[test]\npub fn unit() {}\npub fn plain() {}\nimpl Holder {\n    pub fn mejr() {\n    }\n}\n";
    store
        .put_source("src/guard.rs", source, 0, &language)
        .expect("put");
    // The own flags a fresh extract carries: items first, then
    // methods, the insert order.
    let fresh = language.extract(source).expect("fresh extract");
    let own = fresh
        .items
        .iter()
        .map(|item| item.is_test)
        .chain(fresh.methods.iter().map(|method| method.is_test))
        .collect::<Vec<_>>();
    assert_eq!(own, vec![true, false, false]);
    let graph = store.write_crate_graph(
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
            root: "src/guard.rs".to_owned(),
        }],
        &[phantom_fractal_lang::Module {
            id: 1,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: "src/guard.rs".to_owned(),
            scope: String::new(),
        }],
        &[1],
    );
    graph.expect("graph");
    // The test-module mark lands on every symbol of the module.
    let marked = store.read_items("src/guard.rs").expect("marked items");
    assert!(marked.iter().all(|item| item.is_test), "{marked:?}");
    // The reset restores each symbol to its own flag.
    store
        .reset_symbol_test_flags("src/guard.rs", &own)
        .expect("reset");
    let restored = store.read_items("src/guard.rs").expect("restored items");
    assert_eq!(
        restored.iter().map(|item| item.is_test).collect::<Vec<_>>(),
        vec![true, false]
    );
    // A list that does not match the file's stored symbols fails
    // before anything is written.
    match store.reset_symbol_test_flags("src/guard.rs", &[true, false]) {
        Err(crate::StoreError::OwnFlagCount { stored, flags }) => {
            assert_eq!((stored, flags), (3, 2));
        }
        other => panic!("{other:?}"),
    }
    // A path with no file row stores no symbols: an empty list is a
    // no-op, a non-empty one fails.
    store
        .reset_symbol_test_flags("src/ghost.rs", &[])
        .expect("no-op for an unindexed path");
    match store.reset_symbol_test_flags("src/ghost.rs", &[true]) {
        Err(crate::StoreError::OwnFlagCount { stored, flags }) => {
            assert_eq!((stored, flags), (0, 1));
        }
        other => panic!("{other:?}"),
    }
    drop(store);
    drop(language);
}

pub(super) struct GoFixtureLang {
    entries: std::collections::BTreeMap<&'static str, phantom_fractal_lang::Extract>,
}

impl GoFixtureLang {
    pub(super) fn new(entries: Vec<(&'static str, phantom_fractal_lang::Extract)>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }
}

impl phantom_fractal_lang::Language for GoFixtureLang {
    fn name(&self) -> &'static str {
        "go"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }
    fn qualifier_separator(&self) -> &'static str {
        "."
    }
    fn extract(
        &self,
        source: &str,
    ) -> Result<phantom_fractal_lang::Extract, phantom_fractal_lang::ExtractError> {
        Ok(self.entries.get(source).cloned().unwrap_or_default())
    }
}

pub(super) fn go_item(
    kind: phantom_fractal_lang::ItemKind,
    name: &str,
    line: u32,
) -> phantom_fractal_lang::Item {
    phantom_fractal_lang::Item {
        kind,
        name: name.to_owned(),
        visibility: phantom_fractal_lang::Visibility::Public,
        start_line: line,
        end_line: line,
        signature: String::new(),
        doc_summary: None,
        is_test: false,
    }
}

pub(super) fn go_use(path: &str, alias: Option<&str>, line: u32) -> phantom_fractal_lang::Import {
    phantom_fractal_lang::Import {
        path: path.to_owned(),
        alias: alias.map(str::to_owned),
        glob: false,
        start_line: line,
        visibility: phantom_fractal_lang::Visibility::Private,
    }
}

pub(super) fn go_ref(
    kind: phantom_fractal_lang::RefKind,
    text: &str,
    last: &str,
    line: u32,
    caller: &str,
    caller_line: u32,
) -> phantom_fractal_lang::Reference {
    phantom_fractal_lang::Reference {
        kind,
        text: text.to_owned(),
        last_segment: last.to_owned(),
        line,
        col: 1,
        scope: String::new(),
        from: Some(phantom_fractal_lang::Caller {
            name: caller.to_owned(),
            owner: None,
            start_line: caller_line,
        }),
        receiver: None,
        receiver_type: None,
        shadowed: false,
    }
}

#[test]
fn source_skipped_mirrors_rs_skipped_meta_key() {
    let db = TempDb::new("srcskip");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert_eq!(store.source_skipped().expect("unset"), None);
    store
        .set_workspace_fingerprint(&[0xabu8; 32], 11)
        .expect("set");
    assert_eq!(store.source_skipped().expect("set"), Some(11));
    assert_eq!(store.rs_skipped().expect("compat"), Some(11));
}

#[test]
fn multi_file_go_package_populates_file_and_symbol_module_ids_and_display() {
    use phantom_fractal_lang::{
        Extract, FileModule, ItemKind, Module, RefKind, Target, TargetKind,
    };

    let db = TempDb::new("gomod");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");

    let src_a = "// pkg/a.go";
    let src_b = "// pkg/b.go";
    let go = GoFixtureLang::new(vec![
        (
            src_a,
            Extract {
                items: vec![
                    go_item(ItemKind::Struct, "Decoder", 2),
                    go_item(ItemKind::Function, "Decode", 5),
                ],
                item_scopes: vec![String::new(), String::new()],
                item_owners: vec![None, Some("Decoder".to_owned())],
                imports: vec![go_use("fmt", None, 1)],
                import_scopes: vec![String::new()],
                refs: vec![go_ref(RefKind::Call, "Helper", "Helper", 6, "Decode", 5)],
                ..Extract::default()
            },
        ),
        (
            src_b,
            Extract {
                items: vec![go_item(ItemKind::Function, "Helper", 2)],
                item_scopes: vec![String::new()],
                item_owners: vec![None],
                ..Extract::default()
            },
        ),
    ]);
    store.put_source("pkg/a.go", src_a, 1, &go).expect("put a");
    store.put_source("pkg/b.go", src_b, 2, &go).expect("put b");

    let targets = vec![Target {
        id: 1,
        package: "example.com/app".to_owned(),
        name: "pkg".to_owned(),
        kind: TargetKind::Lib,
        root: String::new(),
    }];
    let modules = vec![Module {
        id: 100,
        target: 1,
        parent: None,
        path: "example.com/app/pkg".to_owned(),
        file: String::new(),
        scope: String::new(),
    }];
    let file_modules = vec![
        FileModule {
            file: "pkg/a.go".to_owned(),
            module: 100,
        },
        FileModule {
            file: "pkg/b.go".to_owned(),
            module: 100,
        },
        FileModule {
            file: "pkg/unindexed.go".to_owned(),
            module: 100,
        },
    ];
    store
        .write_crate_graph_with_files(
            &[Package {
                name: "example.com/app".to_owned(),
                version: None,
                manifest_path: "go.mod".to_owned(),
            }],
            &targets,
            &modules,
            &file_modules,
            &[100],
        )
        .expect("write go graph");

    // Bad FileModule module id must error atomically.
    assert!(matches!(
        store.write_crate_graph_with_files(
            &[],
            &[],
            &[],
            &[FileModule {
                file: "pkg/a.go".to_owned(),
                module: 999,
            }],
            &[],
        ),
        Err(StoreError::UnknownTarget(999))
    ));

    let resolve = store.resolve_input().expect("resolve");
    assert_eq!(resolve.modules.len(), 1);
    assert_eq!(resolve.modules[0].file, "");
    assert!(resolve.uses.iter().all(|u| u.module == Some(1)));
    assert!(resolve.refs.iter().all(|r| r.module == Some(1)));

    // Both pkg/a.go and pkg/b.go files + symbols carry module_id = 1 and is_test = 1.
    let conn = rusqlite::Connection::open(db.path()).expect("sqlite");
    let file_mods: Vec<(String, Option<i64>)> = conn
        .prepare("SELECT path, module_id FROM files ORDER BY path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        file_mods,
        vec![
            ("pkg/a.go".to_owned(), Some(1)),
            ("pkg/b.go".to_owned(), Some(1))
        ]
    );
    let file_id_a: i64 = conn
        .query_row("SELECT id FROM files WHERE path = 'pkg/a.go'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(store.file_has_module(file_id_a).expect("has module"));

    for item in store
        .read_items("pkg/a.go")
        .unwrap()
        .into_iter()
        .chain(store.read_items("pkg/b.go").unwrap())
    {
        assert!(
            item.is_test,
            "test_modules marks all files in package: {item:?}"
        );
    }

    let decode_hits = store.symbols_matching("Decode").expect("decode");
    assert_eq!(decode_hits.len(), 1);
    assert_eq!(
        store
            .format_symbol_display(&decode_hits[0])
            .expect("go display"),
        "pkg/a.go::Decoder.Decode"
    );

    let mut missing_hit = decode_hits[0].clone();
    missing_hit.file = "missing.rs".to_owned();
    assert_eq!(
        store
            .format_symbol_display(&missing_hit)
            .expect("fallback display"),
        "missing.rs::Decoder::Decode"
    );
}

#[test]
fn rust_module_ids_match_legacy_write_crate_graph() {
    let db1 = TempDb::new("rustid1");
    let db2 = TempDb::new("rustid2");
    assert_outside_workspace(db1.path());
    assert_outside_workspace(db2.path());
    let mut s1 = Store::open(db1.path()).expect("s1");
    let mut s2 = Store::open(db2.path()).expect("s2");
    let lang = language();
    let lib = "pub fn root_item() {}
#[cfg(test)]
mod tests { pub fn probe() {} }
";
    let ledger = "pub fn open_ledger() {}
#[cfg(test)]
mod tests { pub fn check() {} }
";
    for store in [&mut s1, &mut s2] {
        put(store, &lang, "src/lib.rs", lib);
        put(store, &lang, "src/ledger.rs", ledger);
    }
    let packages = vec![Package {
        name: "sample".to_owned(),
        version: None,
        manifest_path: "Cargo.toml".to_owned(),
    }];
    let targets = vec![phantom_fractal_lang::Target {
        id: 1,
        package: "sample".to_owned(),
        name: "sample".to_owned(),
        kind: phantom_fractal_lang::TargetKind::Lib,
        root: "src/lib.rs".to_owned(),
    }];
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
    ];
    s1.write_crate_graph(&packages, &targets, &modules, &[12])
        .expect("s1 graph");
    s2.write_crate_graph_with_files(&packages, &targets, &modules, &[], &[12])
        .expect("s2 graph");

    for q in [
        "root_item",
        "crate::root_item",
        "tests::probe",
        "crate::tests::probe",
        "ledger::tests::check",
        "crate::ledger::tests::check",
    ] {
        assert_eq!(
            s1.symbols_matching(q).unwrap(),
            s2.symbols_matching(q).unwrap()
        );
    }
    let c1 = rusqlite::Connection::open(db1.path()).unwrap();
    let c2 = rusqlite::Connection::open(db2.path()).unwrap();
    let f1: Vec<(String, Option<i64>)> = c1
        .prepare("SELECT path, module_id FROM files ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let f2: Vec<(String, Option<i64>)> = c2
        .prepare("SELECT path, module_id FROM files ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(f1, f2);
    let sy1: Vec<(String, Option<i64>, i64)> = c1
        .prepare("SELECT qual_name, module_id, is_test FROM symbols ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let sy2: Vec<(String, Option<i64>, i64)> = c2
        .prepare("SELECT qual_name, module_id, is_test FROM symbols ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(sy1, sy2);
}

#[test]
fn did_you_mean_handles_go_dots_and_excludes_init_and_blank() {
    use phantom_fractal_lang::{Extract, FileModule, ItemKind, Module, Target, TargetKind};

    let db = TempDb::new("dymgo");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");

    let go_src = "// pkg/helper.go";
    let go = GoFixtureLang::new(vec![(
        go_src,
        Extract {
            items: vec![
                go_item(ItemKind::Function, "Helper", 2),
                go_item(ItemKind::Function, "init", 4),
                go_item(ItemKind::Function, "_", 6),
            ],
            item_scopes: vec![String::new(); 3],
            item_owners: vec![None, None, None],
            ..Extract::default()
        },
    )]);
    store
        .put_source("pkg/helper.go", go_src, 0, &go)
        .expect("put go");
    put(
        &mut store,
        &language(),
        "src/init.rs",
        "pub fn init() {}
",
    );

    store
        .write_crate_graph_with_files(
            &[Package {
                name: "example.com/mod".to_owned(),
                version: None,
                manifest_path: "go.mod".to_owned(),
            }],
            &[Target {
                id: 1,
                package: "example.com/mod".to_owned(),
                name: "pkg".to_owned(),
                kind: TargetKind::Lib,
                root: String::new(),
            }],
            &[Module {
                id: 1,
                target: 1,
                parent: None,
                path: "example.com/mod/pkg".to_owned(),
                file: String::new(),
                scope: String::new(),
            }],
            &[FileModule {
                file: "pkg/helper.go".to_owned(),
                module: 1,
            }],
            &[],
        )
        .expect("graph");

    let dot_hits = store.did_you_mean("pkg.Helper").expect("dot");
    assert_eq!(dot_hits.len(), 1);
    assert_eq!(dot_hits[0].file, "pkg/helper.go");

    let mixed_hits = store
        .did_you_mean("example.com/mod/pkg::Type.Helper")
        .expect("mixed");
    assert_eq!(mixed_hits.len(), 1);

    // Go init and _ are excluded from did_you_mean, while Rust init remains.
    let init_hits = store.did_you_mean("pkg.init").expect("init");
    assert_eq!(init_hits.len(), 1);
    assert_eq!(init_hits[0].file, "src/init.rs");
    assert!(store.did_you_mean("_").expect("blank").is_empty());
}

#[test]
fn rust_shared_file_multi_mount_answers_exact_and_scoped_lookups() {
    use phantom_fractal_lang::{Module, Target, TargetKind};
    let db = TempDb::new("multi-mount");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    store
        .put_source(
            "src/shared.rs",
            "pub fn helper() {}\npub struct Widget;\nimpl Widget { pub fn turn(&self) {} }\n",
            0,
            &lang,
        )
        .expect("put");
    store
        .write_crate_graph(
            &[Package {
                name: "probe_pkg".to_owned(),
                version: Some("0.1.0".to_owned()),
                manifest_path: "Cargo.toml".to_owned(),
            }],
            &[
                Target {
                    id: 1,
                    package: "probe_pkg".to_owned(),
                    name: "probe_pkg".to_owned(),
                    kind: TargetKind::Lib,
                    root: "src/lib.rs".to_owned(),
                },
                Target {
                    id: 2,
                    package: "probe_pkg".to_owned(),
                    name: "tool".to_owned(),
                    kind: TargetKind::Bin,
                    root: "src/bin/tool.rs".to_owned(),
                },
            ],
            &[
                Module {
                    id: 1,
                    target: 1,
                    parent: None,
                    path: "crate::shared".to_owned(),
                    file: "src/shared.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 2,
                    target: 1,
                    parent: None,
                    path: "crate::nested::again".to_owned(),
                    file: "src/shared.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 3,
                    target: 2,
                    parent: None,
                    path: "crate::helpers".to_owned(),
                    file: "src/shared.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            &[],
        )
        .expect("graph");

    for q in [
        "helpers::helper",
        "crate::helpers::helper",
        "nested::again::helper",
        "crate::nested::again::helper",
        "helpers::Widget::turn",
        "crate::helpers::Widget::turn",
        "tool::helpers::helper",
        "tool::helpers::Widget::turn",
        "probe_pkg::nested::again::helper",
    ] {
        assert_eq!(store.symbols_matching(q).expect(q).len(), 1, "{q}");
    }
}
