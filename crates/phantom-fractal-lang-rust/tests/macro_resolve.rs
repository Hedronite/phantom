//! `MacroArg` refs placed by their shape over hand-built inputs
//! (SPEC-w12 G1b, D5).

mod support;

use phantom_fractal_lang::{Id, Reason, RefKind, ResolveCounts, SymbolKind};
use support::{Input, edge};

/// A `MacroArg` of `text` from `from`, with the receiver and hint given.
fn macro_arg(
    i: &mut Input,
    from: Option<Id>,
    text: &str,
    receiver: Option<&str>,
    hint: Option<&str>,
) -> Id {
    let id = i.call_in(from, "src/lib.rs", "", text, RefKind::MacroArg);
    let call = i.input.refs.last_mut().expect("ref");
    call.receiver = receiver.map(str::to_owned);
    call.receiver_type = hint.map(str::to_owned);
    id
}

/// Only placed refs are counted, and only in `edges`.
fn placed_only(edges: u64) -> ResolveCounts {
    ResolveCounts {
        edges,
        ..ResolveCounts::default()
    }
}

#[test]
fn a_path_shape_takes_the_call_tiers() {
    let mut i = Input::new();
    let caller = i.function("caller");
    let helper = i.function("helper");
    let local = macro_arg(&mut i, Some(caller), "helper", None, None);
    let exact = macro_arg(&mut i, Some(caller), "crate::helper", None, None);
    macro_arg(&mut i, Some(caller), "std::mem::drop", None, None);
    macro_arg(&mut i, Some(caller), "missing", None, None);
    macro_arg(&mut i, None, "helper", None, None);
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(local, caller, helper, Reason::Local),
            edge(exact, caller, helper, Reason::PathExact),
        ]
    );
    assert_eq!(counts, placed_only(2));
}

#[test]
fn a_shadowed_path_shape_gets_no_edge() {
    let mut i = Input::new();
    let caller = i.function("caller");
    i.function("helper");
    macro_arg(&mut i, Some(caller), "helper", None, None);
    i.input.refs.last_mut().expect("ref").shadowed = true;
    let (edges, counts) = i.resolve();
    assert_eq!(edges, []);
    assert_eq!(counts, ResolveCounts::default());
}

#[test]
fn a_path_shape_never_takes_the_name_rule() {
    // `settle(` is a path, so the one workspace method `settle` is not it.
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.method("Ledger", None, "settle");
    let caller = i.function("caller");
    macro_arg(&mut i, Some(caller), "settle", None, None);
    let (edges, counts) = i.resolve();
    assert_eq!(edges, []);
    assert_eq!(counts, ResolveCounts::default());
}

#[test]
fn a_self_receiver_takes_self_method() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    let total = i.method("Ledger", None, "total");
    let audit = i.method("Ledger", None, "audit");
    let free = i.function("free");
    let placed = macro_arg(&mut i, Some(audit), "total", Some("self"), None);
    macro_arg(&mut i, Some(audit), "missing", Some("self"), None);
    macro_arg(&mut i, Some(free), "total", Some("self"), None);
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(placed, audit, total, Reason::SelfMethod)]);
    assert_eq!(counts, placed_only(1));
}

#[test]
fn a_hinted_receiver_takes_typed_method_and_nothing_else() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Book", "src/lib.rs", "");
    let total = i.method("Ledger", None, "total");
    i.method("Book", None, "total");
    i.method("Ledger", None, "settle");
    let caller = i.function("caller");
    let typed = macro_arg(
        &mut i,
        Some(caller),
        "total",
        Some("x"),
        Some("type:Ledger"),
    );
    // A hint that places nothing never falls back to the name rule.
    macro_arg(&mut i, Some(caller), "settle", Some("v"), Some("type:Vec"));
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(typed, caller, total, Reason::TypedMethod)]);
    assert_eq!(counts, placed_only(1));
}

#[test]
fn any_other_method_shape_takes_only_the_name_rule() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Book", "src/lib.rs", "");
    let settle = i.method("Ledger", None, "settle");
    i.method("Ledger", None, "len");
    i.method("Ledger", None, "label");
    i.method("Book", None, "label");
    let caller = i.function("caller");
    let chained = macro_arg(&mut i, Some(caller), "settle", Some("make()"), None);
    let unhinted = macro_arg(&mut i, Some(caller), "settle::<u8>", Some("x"), None);
    let empty = macro_arg(&mut i, Some(caller), "settle", Some(""), None);
    macro_arg(&mut i, Some(caller), "len", Some("make()"), None);
    macro_arg(&mut i, Some(caller), "label", Some("x"), None);
    macro_arg(&mut i, None, "settle", Some("x"), None);
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(chained, caller, settle, Reason::UniqueMethod),
            edge(unhinted, caller, settle, Reason::UniqueMethod),
            edge(empty, caller, settle, Reason::UniqueMethod),
        ]
    );
    assert_eq!(counts, placed_only(3));
}

#[test]
fn ordinary_refs_place_exactly_as_before_beside_macro_args() {
    let mut i = Input::new();
    let caller = i.function("caller");
    let helper = i.function("helper");
    let call = i.call(caller, "helper");
    macro_arg(&mut i, Some(caller), "missing", None, None);
    i.method_call(Some(caller), "x", "nothing");
    i.call_in(Some(caller), "src/lib.rs", "", "assert", RefKind::Macro);
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(call, caller, helper, Reason::Local)]);
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 1,
            method_calls: 1,
            macros: 1,
            ..ResolveCounts::default()
        }
    );
}
