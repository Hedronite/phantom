//! Crate-graph API rows (G1c): write, resolve, replace, match, and the
//! caller counters.

use phantom_fractal_lang::{Edge, Reason, ResolveInput};
use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace, fixture};
use crate::{Package, Store};

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
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
        .write_crate_graph(&packages, &targets, &modules)
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
        .write_crate_graph(std::slice::from_ref(&package), &targets, &modules)
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
