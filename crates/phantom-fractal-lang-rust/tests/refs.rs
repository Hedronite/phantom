//! Call references over `fixtures/extract/refs.rs` and inline sources
//! (SPEC-w3 G0a).

use phantom_fractal_lang::{Caller, Extract, Language, RefKind, Reference};
use phantom_fractal_lang_rust::RustLanguage;

const REFS: &str = include_str!("../../../fixtures/extract/refs.rs");

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

fn caller(name: &str, owner: Option<&str>, start_line: u32) -> Option<Caller> {
    Some(Caller {
        name: name.to_owned(),
        owner: owner.map(str::to_owned),
        start_line,
    })
}

/// Expected fields of one reference, in `Reference` order.
struct Want<'a> {
    kind: RefKind,
    text: &'a str,
    last: &'a str,
    at: (u32, u32),
    scope: &'a str,
    from: Option<Caller>,
    receiver: Option<&'a str>,
    receiver_type: Option<&'a str>,
    shadowed: bool,
}

impl Want<'_> {
    fn reference(&self) -> Reference {
        Reference {
            kind: self.kind,
            text: self.text.to_owned(),
            last_segment: self.last.to_owned(),
            line: self.at.0,
            col: self.at.1,
            scope: self.scope.to_owned(),
            from: self.from.clone(),
            receiver: self.receiver.map(str::to_owned),
            receiver_type: self.receiver_type.map(str::to_owned),
            shadowed: self.shadowed,
        }
    }
}

fn call<'a>(text: &'a str, last: &'a str, at: (u32, u32), from: Option<Caller>) -> Want<'a> {
    Want {
        kind: RefKind::Call,
        text,
        last,
        at,
        scope: "",
        from,
        receiver: None,
        receiver_type: None,
        shadowed: false,
    }
}

#[test]
fn refs_fixture_gives_every_call_with_its_fields() {
    let plain = || caller("plain", None, 4);
    let new = || caller("new", Some("Ledger"), 18);
    let wants = [
        call("helper", "helper", (5, 5), plain()),
        call("crate::helpers::helper", "helper", (6, 5), plain()),
        call("Ledger::new", "new", (7, 5), plain()),
        call("Vec::<u8>::with_capacity", "with_capacity", (8, 5), plain()),
        Want {
            kind: RefKind::MethodCall,
            receiver: Some("input"),
            ..call("trim", "trim", (9, 24), plain())
        },
        Want {
            kind: RefKind::MethodCall,
            receiver: Some("input.trim()"),
            ..call("parse::<u32>", "parse", (9, 31), plain())
        },
        Want {
            kind: RefKind::Macro,
            ..call("println", "println", (10, 5), plain())
        },
        Want {
            kind: RefKind::MacroArg,
            ..call("hidden_in_macro", "hidden_in_macro", (10, 20), plain())
        },
        call("std::mem::drop", "drop", (11, 5), plain()),
        call("helper_in_closure", "helper_in_closure", (19, 30), new()),
        Want {
            shadowed: true,
            ..call("build", "build", (20, 9), new())
        },
        call("first_byte", "first_byte", (25, 26), None),
        Want {
            scope: "tests",
            ..call("super::plain", "plain", (29, 9), caller("checks", None, 28))
        },
        call(
            "say_hello",
            "say_hello",
            (35, 9),
            caller("greet", Some("Greet"), 34),
        ),
    ];
    let expected: Vec<Reference> = wants.iter().map(Want::reference).collect();
    assert_eq!(extract(REFS).refs, expected);
}

#[test]
fn a_call_inside_a_macro_token_tree_is_only_a_macro_arg() {
    let got = extract(REFS);
    let kinds: Vec<RefKind> = got
        .refs
        .iter()
        .filter(|r| r.text == "hidden_in_macro")
        .map(|r| r.kind)
        .collect();
    assert_eq!(kinds, [RefKind::MacroArg]);
}

/// The `shadowed` flag of the one call to `g` in `source`.
fn g_shadowed(source: &str) -> bool {
    let got = extract(source);
    let calls: Vec<&Reference> = got
        .refs
        .iter()
        .filter(|r| r.last_segment == "g" && r.kind != RefKind::Macro)
        .collect();
    assert_eq!(calls.len(), 1, "{source}: {calls:?}");
    calls[0].shadowed
}

#[test]
fn every_binding_form_shadows_a_single_segment_call() {
    let cases = [
        ("let", "fn f() { let g = 1; g(); }"),
        ("let tuple", "fn f() { let (a, g) = pair; g(); }"),
        ("parameter", "fn f(g: fn()) { g(); }"),
        ("parameter pattern", "fn f((g, b): (fn(), u8)) { g(); }"),
        ("closure parameter", "fn f() { let c = |g| g(); }"),
        (
            "typed closure parameter",
            "fn f() { let c = |g: fn()| g(); }",
        ),
        (
            "match",
            "fn f(x: Option<fn()>) { match x { Some(g) => g(), None => {} } }",
        ),
        (
            "match binding",
            "fn f(x: u8) { match x { g @ 1 => g(), _ => {} } }",
        ),
        ("for", "fn f(v: Vec<fn()>) { for g in v { g(); } }"),
        (
            "if let",
            "fn f(o: Option<fn()>) { if let Some(g) = o { g(); } }",
        ),
        (
            "while let",
            "fn f(mut o: I) { while let Some(g) = o.next() { g(); } }",
        ),
        (
            "struct shorthand",
            "fn f(s: S) { let S { g, .. } = s; g(); }",
        ),
        ("struct field", "fn f(s: S) { let S { a: g } = s; g(); }"),
        ("local use", "fn f() { use a::b as g; g(); }"),
        ("local use leaf", "fn f() { use a::{b, g}; g(); }"),
        ("local fn", "fn f() { fn g() {} g(); }"),
        ("let after the call", "fn f() { g(); let g = 1; }"),
        (
            "let in a closure",
            "fn f() { let c = || { let g = 1; }; g(); }",
        ),
        ("generic call", "fn f(g: fn()) { g::<u8>(); }"),
    ];
    for (form, source) in cases {
        assert!(g_shadowed(source), "{form}: {source}");
    }
}

#[test]
fn unbound_paths_methods_and_other_functions_are_not_shadowed() {
    let cases = [
        ("unbound", "fn f() { let x = 1; g(); }"),
        ("path", "fn f() { let g = 1; a::g(); }"),
        ("method", "fn f(g: G) { g.g(); }"),
        (
            "variant path",
            "fn f(x: E) { match x { E::g => {}, _ => {} } g(); }",
        ),
        ("tuple struct type", "fn f(x: W) { let g(a) = x; g(); }"),
        (
            "guard",
            "fn f(x: Option<u8>) { match x { Some(y) if g(y) => {}, _ => {} } }",
        ),
        (
            "nested fn binding",
            "fn f() { fn inner() { let g = 1; } g(); }",
        ),
        ("other function", "fn h(g: u8) {}\nfn f() { g(); }"),
        ("glob use", "fn f() { use a::*; g(); }"),
        ("outside any function", "static X: u8 = g();"),
    ];
    for (form, source) in cases {
        assert!(!g_shadowed(source), "{form}: {source}");
    }
}

#[test]
fn a_nested_fn_and_its_closures_are_the_caller() {
    let got = extract("fn outer() { fn inner() { let c = || g(); } h(); }");
    let from = |name: &str| {
        got.refs
            .iter()
            .find(|r| r.last_segment == name)
            .and_then(|r| r.from.clone())
    };
    assert_eq!(from("g"), caller("inner", None, 1));
    assert_eq!(from("h"), caller("outer", None, 1));
}

#[test]
fn other_callee_shapes_give_no_reference() {
    let got = extract("fn f() { (g)(); h()(); fns[0](); }");
    let texts: Vec<&str> = got.refs.iter().map(|r| r.text.as_str()).collect();
    // `(g)()` and the outer call of `h()()` have no path callee. The inner
    // `h()` does.
    assert_eq!(texts, ["h"]);
}

#[test]
fn scoped_macro_and_whitespace_are_compacted() {
    let got = extract("fn f() {\n    std :: println!(\"x\");\n    a :: < u8 > :: b ();\n}\n");
    let texts: Vec<(&str, &str, RefKind)> = got
        .refs
        .iter()
        .map(|r| (r.text.as_str(), r.last_segment.as_str(), r.kind))
        .collect();
    assert_eq!(
        texts,
        [
            ("std::println", "println", RefKind::Macro),
            ("a::<u8>::b", "b", RefKind::Call),
        ]
    );
}

#[test]
fn a_deeply_nested_expression_does_not_overflow() {
    let depth = 20_000;
    let source = format!("fn f() {{ {}g(){} }}", "(".repeat(depth), ")".repeat(depth));
    let got = extract(&source);
    assert!(got.refs.iter().any(|r| r.text == "g"));
}

#[test]
fn the_caller_start_line_tells_same_named_functions_apart() {
    let source = "\
pub trait Greet {
    fn greet(&self) {
        a();
    }
}

impl Greet for A {
    #[inline]
    fn greet(&self) {
        b();
    }
}

fn outer() {
    fn inner() {
        c();
    }
}

fn inner() {
    d();
}

#[cfg(unix)]
fn twin() {
    e();
}

#[cfg(not(unix))]
fn twin() {
    f();
}
";
    let got = extract(source);
    let from = |text: &str| {
        got.refs
            .iter()
            .find(|r| r.text == text)
            .and_then(|r| r.from.clone())
    };
    assert_eq!(from("a"), caller("greet", Some("Greet"), 2));
    assert_eq!(from("b"), caller("greet", Some("A"), 9));
    assert_eq!(from("c"), caller("inner", None, 15));
    assert_eq!(from("d"), caller("inner", None, 20));
    assert_eq!(from("e"), caller("twin", None, 25));
    assert_eq!(from("f"), caller("twin", None, 30));
    // The impl method's caller line is its `Method` record's line.
    let method = got.methods.iter().find(|m| m.owner == "A").expect("method");
    assert_eq!(method.start_line, 9);
    let twins: Vec<u32> = got
        .items
        .iter()
        .filter(|item| item.name == "twin")
        .map(|item| item.start_line)
        .collect();
    assert_eq!(twins, [25, 30]);
}
