//! The `unique-method` tier over hand-built inputs (SPEC-w10 G3a, D6).

mod support;

use phantom_fractal_lang::{Reason, ResolveCounts, SymbolKind, TargetKind};
use phantom_fractal_lang_rust::STD_METHOD_NAMES;
use support::{Input, edge, only_edge};

#[test]
fn the_stoplist_is_sorted_without_duplicates() {
    assert!(
        STD_METHOD_NAMES.windows(2).all(|pair| pair[0] < pair[1]),
        "not strictly sorted"
    );
    for name in [
        "len",
        "iter",
        "push",
        "get",
        "insert",
        "clone",
        "map",
        "unwrap",
        "collect",
        "next",
        "as_ref",
        "to_string",
    ] {
        assert!(STD_METHOD_NAMES.binary_search(&name).is_ok(), "{name}");
    }
    assert!(STD_METHOD_NAMES.iter().all(|name| {
        !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    }));
}

#[test]
fn an_unhinted_call_of_a_unique_name_places_it_at_half() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    let settle = i.method("Ledger", None, "settle");
    let f = i.function("f");
    let plain = i.method_call(Some(f), "make()", "settle");
    let generic = i.method_call(Some(f), "x", "settle::<u8>");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(plain, f, settle, Reason::UniqueMethod),
            edge(generic, f, settle, Reason::UniqueMethod),
        ]
    );
    assert!(edges.iter().all(|edge| edge.confidence == 0.5));
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 2,
            ..ResolveCounts::default()
        }
    );
}

#[test]
fn a_stoplisted_name_gets_nothing() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    let f = i.function("f");
    for name in ["len", "get", "to_string", "unwrap"] {
        i.method("Ledger", None, name);
        i.method_call(Some(f), "x", name);
    }
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.method_calls, 4);
}

#[test]
fn a_name_defined_twice_anywhere_gets_nothing() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Book", "src/lib.rs", "");
    i.symbol(SymbolKind::Trait, "Tag", "src/lib.rs", "");
    i.method("Ledger", None, "label");
    i.method("Book", None, "label");
    // A trait impl, and an impl on a type no lookup finds, count as well.
    i.method("Ledger", Some("Tag"), "mark");
    i.method("Missing", None, "mark");
    let f = i.function("f");
    i.method_call(Some(f), "x", "label");
    i.method_call(Some(f), "x", "mark");
    // A free function of the name is not a method.
    i.function("only");
    let only = i.method("Ledger", None, "only");
    let call = i.method_call(Some(f), "x", "only");
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(call, f, only, Reason::UniqueMethod)]);
    assert_eq!(counts.method_calls, 2);
}

#[test]
fn the_one_method_may_sit_in_another_crate() {
    let mut i = Input::new();
    i.target("geo", TargetKind::Lib, "geo/src/lib.rs");
    i.symbol(SymbolKind::Struct, "Point", "geo/src/lib.rs", "");
    let norm = i.method_in("geo/src/lib.rs", "Point", None, "norm");
    let f = i.function("f");
    let call = i.method_call(Some(f), "p", "norm");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, f, norm, Reason::UniqueMethod))
    );
}

#[test]
fn a_hinted_call_never_takes_the_name_rule() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    let settle = i.method("Ledger", None, "settle");
    let f = i.function("f");
    let typed = i.typed_call(f, "l", "settle", "type:Ledger");
    for hint in ["type:Vec", "type:Missing", "ctor:Ledger::absent"] {
        i.typed_call(f, "v", "settle", hint);
    }
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(typed, f, settle, Reason::TypedMethod)]);
    assert_eq!(counts.method_calls, 3);
}

#[test]
fn self_and_sourceless_calls_never_take_the_name_rule() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Book", "src/lib.rs", "");
    i.method("Ledger", None, "settle");
    let shelve = i.method("Book", None, "shelve");
    i.method_call(Some(shelve), "self", "settle");
    i.method_call(None, "x", "settle");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(
        counts,
        ResolveCounts {
            unresolved: 1,
            method_calls: 1,
            ..ResolveCounts::default()
        }
    );
}
