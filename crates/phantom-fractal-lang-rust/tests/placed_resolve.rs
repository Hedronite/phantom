//! The D6 hints placed over hand-built inputs (SPEC-w12 G2a), and the base
//! tier first.

mod support;

use phantom_fractal_lang::{Id, Reason, SymbolKind};
use support::{Input, edge, only_edge};

/// `Ledger` and `Book`, each with a method `label`, so the name alone
/// places nothing.
fn two_types(i: &mut Input) -> (Id, Id) {
    i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Book", "src/lib.rs", "");
    let ledger = i.method("Ledger", None, "label");
    let book = i.method("Book", None, "label");
    (ledger, book)
}

/// A free fn `name` in the root module with `signature`.
fn function(i: &mut Input, name: &str, signature: &str) -> Id {
    let id = i.function(name);
    i.signature(id, signature);
    id
}

/// A method call `receiver.name()` from `from` with the hint `hint`.
fn hinted(i: &mut Input, from: Id, receiver: &str, name: &str, hint: &str) -> Id {
    i.typed_call(from, receiver, name, hint)
}

#[test]
fn a_ret_hint_places_by_the_fn_return_type() {
    for (signature, hint, placed) in [
        ("fn make() -> Book", "ret:make", true),
        ("pub fn make() -> crate::Book", "ret:make", true),
        ("fn make() -> Result<Book, Error>", "ret?:make", true),
        ("fn make() -> Option<Book>", "ret?:make", true),
        ("fn make() -> Result<Book, Error>", "ret:make", false),
        ("fn make() -> Book", "ret?:make", false),
        ("fn make() -> u32", "ret:make", false),
        ("fn make() -> &'static Book", "ret:make", false),
        ("fn make() -> impl Tr", "ret:make", false),
        ("fn make() -> Self", "ret:make", false),
        ("fn make<Book>() -> Book", "ret:make", false),
        ("fn make()", "ret:make", false),
    ] {
        let mut i = Input::new();
        let (_, book) = two_types(&mut i);
        function(&mut i, "make", signature);
        let caller = i.function("caller");
        let call = hinted(&mut i, caller, "x", "label", hint);
        let want = placed.then(|| edge(call, caller, book, Reason::TypedMethod));
        assert_eq!(only_edge(&i), want, "{signature} {hint}");
    }
}

#[test]
fn a_ret_hint_needs_one_workspace_fn() {
    let mut i = Input::new();
    two_types(&mut i);
    let caller = i.function("caller");
    hinted(&mut i, caller, "x", "label", "ret:missing");
    hinted(&mut i, caller, "x", "label", "ret:std::mem::take");
    assert_eq!(only_edge(&i), None);
}

#[test]
fn a_ctor_with_zero_or_multiple_inherent_candidates_places_nothing() {
    let mut i = Input::new();
    let (_, book) = two_types(&mut i);
    // `open` is an inherent method on `Book`, not `Ledger`: hits 0 matches for `Ledger::open`.
    let book_open = i.method("Book", None, "open");
    i.signature(book_open, "pub fn open() -> Self");
    // Two inherent `#[cfg]` methods named `fresh` on `Ledger`: hits 2 matches for `Ledger::fresh`.
    let fresh_a = i.method("Ledger", None, "fresh");
    let fresh_b = i.method("Ledger", None, "fresh");
    i.signature(fresh_a, "pub fn fresh() -> Self");
    i.signature(fresh_b, "pub fn fresh() -> Self");
    let caller = i.function("caller");
    hinted(&mut i, caller, "x", "label", "ctor:Ledger::open");
    hinted(&mut i, caller, "y", "label", "ctor:Ledger::fresh");
    let call = hinted(&mut i, caller, "z", "label", "ctor:Book::open");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, caller, book, Reason::TypedMethod))
    );
}

#[test]
fn a_ctor_places_another_returned_type() {
    let mut i = Input::new();
    let (ledger, book) = two_types(&mut i);
    let shelve = i.method("Ledger", None, "shelve");
    i.signature(shelve, "pub fn shelve() -> Book");
    let open = i.method("Ledger", None, "open");
    i.signature(open, "pub fn open() -> Self");
    let find = i.method("Ledger", None, "find");
    i.signature(find, "pub fn find() -> Option<Book>");
    let caller = i.function("caller");
    let other = hinted(&mut i, caller, "x", "label", "ctor:Ledger::shelve");
    let own = hinted(&mut i, caller, "y", "label", "ctor:Ledger::open");
    let tried = hinted(&mut i, caller, "z", "label", "ctor?:Ledger::find");
    let (edges, _) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(other, caller, book, Reason::TypedMethod),
            edge(own, caller, ledger, Reason::TypedMethod),
            edge(tried, caller, book, Reason::TypedMethod),
        ]
    );
}

#[test]
fn a_ctor_return_that_is_a_generic_of_the_method_or_impl_places_nothing() {
    for (owner, signature) in [
        ("Ledger", "fn into<Book>(self) -> Book"),
        ("Ledger<Book>", "fn inner(&self) -> Book"),
    ] {
        let mut i = Input::new();
        two_types(&mut i);
        let into = i.method(owner, None, "into");
        i.signature(into, signature);
        let caller = i.function("caller");
        hinted(&mut i, caller, "x", "label", "ctor:Ledger::into");
        assert_eq!(only_edge(&i), None, "{owner} {signature}");
    }
}

#[test]
fn a_ctor_of_a_module_path_places_as_a_ret() {
    let mut i = Input::new();
    let (_, book) = two_types(&mut i);
    i.module(1, "helpers", "src/lib.rs", "helpers");
    let make = i.symbol(SymbolKind::Function, "make", "src/lib.rs", "helpers");
    i.signature(make, "pub fn make() -> crate::Book");
    let caller = i.function("caller");
    let call = hinted(&mut i, caller, "x", "label", "ctor:helpers::make");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, caller, book, Reason::TypedMethod))
    );
}

#[test]
fn a_new_hint_is_tried_only_when_the_name_alone_places_nothing() {
    let mut i = Input::new();
    let (_, book) = two_types(&mut i);
    i.symbol(SymbolKind::Struct, "Other", "src/lib.rs", "");
    let settle = i.method("Other", None, "settle");
    i.method("Book", None, "settle_twice");
    function(&mut i, "make", "fn make() -> Book");
    let caller = i.function("caller");
    // `settle` is unique: each new-hint shape keeps the unique edge, even
    // though its hint names `Book`.
    let by_ret = hinted(&mut i, caller, "x", "settle", "ret:make");
    let by_call = hinted(&mut i, caller, "make()", "settle", "ret:make");
    let by_field = hinted(&mut i, caller, "self.f", "settle", "type:Book");
    let by_ctor_call = hinted(&mut i, caller, "Book::new()", "settle", "ctor:Book::new");
    // `label` is not unique: the new hint places it.
    let typed = hinted(&mut i, caller, "self.f", "label", "type:Book");
    // A base hint on one identifier is typed only, as before.
    hinted(&mut i, caller, "y", "settle", "type:Book");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(by_ret, caller, settle, Reason::UniqueMethod),
            edge(by_call, caller, settle, Reason::UniqueMethod),
            edge(by_field, caller, settle, Reason::UniqueMethod),
            edge(by_ctor_call, caller, settle, Reason::UniqueMethod),
            edge(typed, caller, book, Reason::TypedMethod),
        ]
    );
    assert_eq!(counts.method_calls, 1);
}
