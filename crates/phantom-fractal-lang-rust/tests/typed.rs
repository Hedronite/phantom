//! The `typed-method` tier over hand-built inputs (SPEC-w10 G2a).

mod support;

use phantom_fractal_lang::{Reason, ResolveCounts, SymbolKind, TargetKind};
use phantom_fractal_lang_rust::confidence;
use support::{Input, edge, only_edge};

/// `Counter` with an inherent `read`, `new` returning `Self`, and the
/// calling function `f`.
fn counter() -> (Input, i64, i64, i64) {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Counter", "src/lib.rs", "");
    let read = i.method("Counter", None, "read");
    let new = i.method("Counter", None, "new");
    i.signature(new, "pub fn new() -> Self");
    let f = i.function("f");
    (i, read, new, f)
}

#[test]
fn the_new_confidences_are_the_spec_constants() {
    assert_eq!(confidence(Reason::TypedMethod), 0.8);
    assert_eq!(confidence(Reason::UniqueMethod), 0.5);
    assert_eq!(Reason::TypedMethod.as_str(), "typed-method");
    assert_eq!(Reason::UniqueMethod.as_str(), "unique-method");
}

#[test]
fn a_type_hint_places_the_one_method_of_its_type() {
    let (mut i, read, _, f) = counter();
    let call = i.typed_call(f, "c", "read", "type:Counter");
    let generic = i.typed_call(f, "c", "read::<u8>", "type:Counter");
    let pathed = i.typed_call(f, "c", "read", "type:crate::Counter");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(call, f, read, Reason::TypedMethod),
            edge(generic, f, read, Reason::TypedMethod),
            edge(pathed, f, read, Reason::TypedMethod),
        ]
    );
    assert!(edges.iter().all(|edge| edge.confidence == 0.8));
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 3,
            ..ResolveCounts::default()
        }
    );
}

#[test]
fn a_trait_impl_method_counts_and_two_candidates_give_nothing() {
    let (mut i, _, _, f) = counter();
    i.symbol(SymbolKind::Trait, "Left", "src/lib.rs", "");
    i.symbol(SymbolKind::Trait, "Right", "src/lib.rs", "");
    let peek = i.method("Counter", Some("Left"), "peek");
    i.method("Counter", Some("Left"), "go");
    i.method("Counter", Some("Right"), "go");
    // An inherent method and a trait method of one name are two too.
    i.method("Counter", Some("Right"), "read");
    let to_trait = i.typed_call(f, "c", "peek", "type:Counter");
    i.typed_call(f, "c", "go", "type:Counter");
    i.typed_call(f, "c", "read", "type:Counter");
    i.typed_call(f, "c", "missing", "type:Counter");
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(to_trait, f, peek, Reason::TypedMethod)]);
    assert_eq!(counts.method_calls, 3);
    assert_eq!(counts.unresolved, 0);
}

#[test]
fn a_hint_must_name_exactly_one_workspace_struct_or_enum() {
    let (mut i, _, _, f) = counter();
    i.symbol(SymbolKind::Enum, "Mode", "src/lib.rs", "");
    let flip = i.method("Mode", None, "flip");
    i.symbol(SymbolKind::Trait, "Tr", "src/lib.rs", "");
    i.method("Tr", None, "read");
    i.symbol(SymbolKind::Function, "read_fn", "src/lib.rs", "");
    let on_enum = i.typed_call(f, "m", "flip", "type:Mode");
    for hint in [
        "type:Tr",
        "type:read_fn",
        "type:Vec",
        "type:std::sync::Arc",
        "type:Missing",
        "type:Counter::read",
        "type:",
        "Counter",
        "kind:Counter",
    ] {
        i.typed_call(f, "c", "read", hint);
    }
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(on_enum, f, flip, Reason::TypedMethod)]);
    assert_eq!(counts.method_calls, 9);
    assert_eq!(counts.edges, 1);
}

#[test]
fn two_types_of_one_name_give_nothing() {
    let (mut i, _, _, f) = counter();
    i.symbol(SymbolKind::Struct, "Counter", "src/lib.rs", "");
    i.typed_call(f, "c", "read", "type:Counter");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.method_calls, 1);
}

#[test]
fn a_ctor_hint_needs_one_inherent_ctor_returning_self_or_the_type() {
    let (mut i, read, _, f) = counter();
    let cases = [
        ("fresh", "pub fn fresh(n: u32) -> Counter", true),
        (
            "generic",
            "pub fn generic<T: Fn() -> u8>(t: T) -> Counter<T> where T: Copy",
            true,
        ),
        ("pathed", "pub fn pathed() -> crate::Counter", true),
        ("count", "pub fn count() -> u32", false),
        ("result", "pub fn result() -> Result<Self, Error>", false),
        ("unit", "pub fn unit()", false),
        ("reference", "pub fn reference(&self) -> &Self", false),
        ("cut", "pub fn cut() -> Sel…", false),
    ];
    let mut want = Vec::new();
    let new_call = i.typed_call(f, "c", "read", "ctor:Counter::new");
    want.push(edge(new_call, f, read, Reason::TypedMethod));
    for (name, signature, placed) in cases {
        let ctor = i.method("Counter", None, name);
        i.signature(ctor, signature);
        let call = i.typed_call(f, "c", "read", &format!("ctor:Counter::{name}"));
        if placed {
            want.push(edge(call, f, read, Reason::TypedMethod));
        }
    }
    // A ctor with no stored signature, one that is not inherent, one that
    // is not there, and one under a type that is not one struct.
    i.method("Counter", None, "unsigned");
    i.typed_call(f, "c", "read", "ctor:Counter::unsigned");
    i.symbol(SymbolKind::Trait, "Make", "src/lib.rs", "");
    let made = i.method("Counter", Some("Make"), "made");
    i.signature(made, "fn made() -> Self");
    i.typed_call(f, "c", "read", "ctor:Counter::made");
    i.typed_call(f, "c", "read", "ctor:Counter::absent");
    i.typed_call(f, "c", "read", "ctor:Missing::new");
    i.typed_call(f, "c", "read", "ctor:new");
    let (edges, _) = i.resolve();
    assert_eq!(edges, want);
}

#[test]
fn a_tried_ctor_needs_a_result_or_option_of_self_or_the_type() {
    let (mut i, read, _, f) = counter();
    let cases = [
        ("open", "pub fn open() -> Result<Self, Error>", true),
        ("named", "pub fn named() -> Result<Counter, Error>", true),
        ("aliased", "pub fn aliased() -> io::Result<Self>", true),
        (
            "nested",
            "pub fn nested() -> Result<Self, Box<dyn Fn() -> Result<u8, E>>>",
            true,
        ),
        ("find", "pub fn find() -> Option<Counter>", true),
        ("plain", "pub fn plain() -> Self", false),
        ("vec", "pub fn vec() -> Vec<Self>", false),
        (
            "option_pair",
            "pub fn option_pair() -> Option<Self, u8>",
            false,
        ),
        ("inner", "pub fn inner() -> Result<u32, Self>", false),
        ("boxed", "pub fn boxed() -> Result<Box<Self>, E>", false),
    ];
    let mut want = Vec::new();
    for (name, signature, placed) in cases {
        let ctor = i.method("Counter", None, name);
        i.signature(ctor, signature);
        let call = i.typed_call(f, "c", "read", &format!("ctor?:Counter::{name}"));
        if placed {
            want.push(edge(call, f, read, Reason::TypedMethod));
        }
    }
    let (edges, _) = i.resolve();
    assert_eq!(edges, want);
}

#[test]
fn a_hint_reaches_a_type_in_another_workspace_crate() {
    let mut i = Input::new();
    i.target("geo", TargetKind::Lib, "geo/src/lib.rs");
    i.dep("pkg", "geo");
    i.symbol(SymbolKind::Struct, "Point", "geo/src/lib.rs", "");
    let norm = i.method_in("geo/src/lib.rs", "Point", None, "norm");
    let new = i.method_in("geo/src/lib.rs", "Point", None, "new");
    i.signature(new, "pub fn new() -> Self");
    i.uses("src/lib.rs", "", "geo::Point", None, false);
    let f = i.function("f");
    let by_use = i.typed_call(f, "p", "norm", "type:Point");
    let by_path = i.typed_call(f, "p", "norm", "type:geo::Point");
    let by_ctor = i.typed_call(f, "p", "norm", "ctor:geo::Point::new");
    let (edges, _) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(by_use, f, norm, Reason::TypedMethod),
            edge(by_path, f, norm, Reason::TypedMethod),
            edge(by_ctor, f, norm, Reason::TypedMethod),
        ]
    );
}

#[test]
fn a_method_of_another_crates_impl_on_the_type_is_not_paired() {
    let mut i = Input::new();
    i.target("geo", TargetKind::Lib, "geo/src/lib.rs");
    i.dep("pkg", "geo");
    i.symbol(SymbolKind::Struct, "Point", "geo/src/lib.rs", "");
    i.symbol(SymbolKind::Trait, "Ext", "src/lib.rs", "");
    // `impl Ext for geo::Point` in the calling crate: `method_owners`
    // pairs no method with a type of another target.
    i.method("geo::Point", Some("Ext"), "ext");
    let f = i.function("f");
    i.typed_call(f, "p", "ext", "type:geo::Point");
    assert_eq!(only_edge(&i), None);
}

#[test]
fn method_calls_counts_only_unplaced_non_self_method_calls() {
    let (mut i, read, _, f) = counter();
    let method = i.method("Counter", None, "touch");
    let typed = i.typed_call(f, "c", "read", "type:Counter");
    i.typed_call(f, "c", "read", "type:Gone");
    i.method_call(Some(f), "c", "read");
    let on_self = i.method_call(Some(method), "self", "read");
    // A hinted call outside any function has no source.
    let outside = i.method_call(None, "c", "read");
    i.input
        .refs
        .iter_mut()
        .find(|r| r.id == outside)
        .expect("ref")
        .receiver_type = Some("type:Counter".to_owned());
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(typed, f, read, Reason::TypedMethod),
            edge(on_self, method, read, Reason::SelfMethod),
        ]
    );
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 2,
            method_calls: 3,
            ..ResolveCounts::default()
        }
    );
}

#[test]
fn a_self_receiver_ignores_any_hint() {
    let (mut i, read, _, _) = counter();
    let method = i.method("Counter", None, "touch");
    let call = i.typed_call(method, "self", "read", "type:Nothing");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, method, read, Reason::SelfMethod))
    );
}
