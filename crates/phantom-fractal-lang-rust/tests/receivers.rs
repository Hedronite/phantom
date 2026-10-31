//! Receiver type hints on method calls (SPEC-w10 G2a, D5).

use phantom_fractal_lang::{Language, RefKind};
use phantom_fractal_lang_rust::RustLanguage;

/// The hint of the one method call named `m` in `source`.
fn hint(source: &str) -> Option<String> {
    let extract = RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses");
    let calls: Vec<_> = extract
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MethodCall && r.last_segment == "m")
        .collect();
    assert_eq!(calls.len(), 1, "{source}: {calls:?}");
    calls[0].receiver_type.clone()
}

fn assert_hints(cases: &[(&str, &str, Option<&str>)]) {
    for (form, source, want) in cases {
        assert_eq!(hint(source).as_deref(), *want, "{form}: {source}");
    }
}

#[test]
fn every_hint_row_gives_its_hint() {
    assert_hints(&[
        ("parameter", "fn f(x: T) { x.m(); }", Some("type:T")),
        ("mut parameter", "fn f(mut x: T) { x.m(); }", Some("type:T")),
        ("reference", "fn f(x: &T) { x.m(); }", Some("type:T")),
        ("lifetime", "fn f<'a>(x: &'a T) { x.m(); }", Some("type:T")),
        (
            "mutable reference",
            "fn f(x: &mut T) { x.m(); }",
            Some("type:T"),
        ),
        ("box", "fn f(x: Box<T>) { x.m(); }", Some("type:T")),
        ("rc", "fn f(x: Rc<T>) { x.m(); }", Some("type:T")),
        ("arc", "fn f(x: Arc<T>) { x.m(); }", Some("type:T")),
        (
            "std arc",
            "fn f(x: std::sync::Arc<T>) { x.m(); }",
            Some("type:T"),
        ),
        ("path", "fn f(x: a::b::T) { x.m(); }", Some("type:a::b::T")),
        (
            "crate path",
            "fn f(x: crate::T) { x.m(); }",
            Some("type:crate::T"),
        ),
        (
            "generic arguments",
            "fn f(x: T<u8, V>) { x.m(); }",
            Some("type:T"),
        ),
        (
            "generic path",
            "fn f(x: &a::T<u8>) { x.m(); }",
            Some("type:a::T"),
        ),
        (
            "annotated let",
            "fn f() { let x: T = g(); x.m(); }",
            Some("type:T"),
        ),
        (
            "annotated mut let",
            "fn f() { let mut x: &mut T = g(); x.m(); }",
            Some("type:T"),
        ),
        (
            "annotated let box",
            "fn f() { let x: Box<T> = g(); x.m(); }",
            Some("type:T"),
        ),
        (
            "annotated let without value",
            "fn f() { let x: T; x.m(); }",
            Some("type:T"),
        ),
        (
            "literal",
            "fn f() { let x = T { a: 1 }; x.m(); }",
            Some("type:T"),
        ),
        (
            "literal path",
            "fn f() { let x = a::T { a: 1 }; x.m(); }",
            Some("type:a::T"),
        ),
        (
            "literal turbofish",
            "fn f() { let x = T::<u8> { a: 1 }; x.m(); }",
            Some("type:T"),
        ),
        (
            "ctor",
            "fn f() { let x = T::new(1); x.m(); }",
            Some("ctor:T::new"),
        ),
        (
            "ctor path",
            "fn f() { let x = a::T::new(); x.m(); }",
            Some("ctor:a::T::new"),
        ),
        (
            "ctor turbofish",
            "fn f() { let x = T::<u8>::new(); x.m(); }",
            Some("ctor:T::new"),
        ),
        (
            "ctor generic fn",
            "fn f() { let x = T::new::<u8>(); x.m(); }",
            Some("ctor:T::new"),
        ),
        (
            "tried ctor",
            "fn f() { let x = T::open()?; x.m(); }",
            Some("ctor?:T::open"),
        ),
        (
            "annotation over value",
            "fn f() { let x: T = U::new(); x.m(); }",
            Some("type:T"),
        ),
    ]);
}

#[test]
fn self_is_the_impl_self_type() {
    assert_hints(&[
        (
            "parameter",
            "impl T { fn f(x: Self) { x.m(); } }",
            Some("type:T"),
        ),
        (
            "reference",
            "impl T { fn f(x: &Self) { x.m(); } }",
            Some("type:T"),
        ),
        (
            "generic impl",
            "impl<A> a::T<A> { fn f(x: Self) { x.m(); } }",
            Some("type:a::T"),
        ),
        (
            "trait impl",
            "impl Tr for T { fn f(x: Self) { x.m(); } }",
            Some("type:T"),
        ),
        (
            "literal",
            "impl T { fn f() { let x = Self { a: 1 }; x.m(); } }",
            Some("type:T"),
        ),
        (
            "ctor",
            "impl T { fn f() { let x = Self::new(); x.m(); } }",
            Some("ctor:T::new"),
        ),
        (
            "tried ctor",
            "impl T { fn f() { let x = Self::new()?; x.m(); } }",
            Some("ctor?:T::new"),
        ),
        (
            "trait default body",
            "trait Tr { fn f(x: Self) { x.m(); } }",
            None,
        ),
        ("free function", "fn f(x: Self) { x.m(); }", None),
        (
            "reference self type",
            "impl Tr for &T { fn f(x: Self) { x.m(); } }",
            None,
        ),
        (
            "associated type",
            "impl T { fn f(x: Self::Item) { x.m(); } }",
            None,
        ),
    ]);
}

#[test]
fn every_other_binding_clears_the_hint() {
    let base = "fn f(x: T, o: O) {";
    let cases = [
        ("untyped let", "let x = g(); x.m();"),
        ("inferred let", "let x: _ = g(); x.m();"),
        ("let without type or value", "let x; x.m();"),
        ("method value", "let x = y.make(); x.m();"),
        ("field value", "let x = y.inner; x.m();"),
        ("unwrapped ctor", "let x = T::new().unwrap(); x.m();"),
        ("free function ctor", "let x = make(); x.m();"),
        ("tried method", "let x = y.make()?; x.m();"),
        ("match", "match o { Some(x) => x.m(), None => {} }"),
        ("match binding", "match o { x @ _ => x.m() }"),
        ("if let", "if let Some(x) = o { x.m(); }"),
        (
            "let chain",
            "if let Some(y) = o && let Some(x) = y { x.m(); }",
        ),
        ("while let", "while let Some(x) = o.next() { x.m(); }"),
        ("for", "for x in o { x.m(); }"),
        ("closure parameter", "let c = |x| x.m();"),
        ("typed closure parameter", "let c = |x: T| x.m();"),
        ("tuple", "let (x, y) = o; x.m();"),
        ("struct", "let S { x } = o; x.m();"),
        ("ref", "let ref x = o; x.m();"),
        ("let else", "let Some(x) = o else { return }; x.m();"),
        ("tuple type", "let x: (T, U) = o; x.m();"),
        ("primitive", "let x: u32 = 1; x.m();"),
        ("impl trait", "let x: impl Tr = o; x.m();"),
        ("dyn trait", "let x: &dyn Tr = o; x.m();"),
        ("two wrappers", "let x: &Box<T> = o; x.m();"),
        ("box of a reference", "let x: Box<&T> = o; x.m();"),
        ("qualified path", "let x = <T as Tr>::new(); x.m();"),
        ("local const", "const x: T = T; x.m();"),
    ];
    for (form, body) in cases {
        let source = format!("{base} {body} }}");
        assert_eq!(hint(&source), None, "{form}: {source}");
    }
}

#[test]
fn a_generic_parameter_is_not_a_type() {
    assert_hints(&[
        ("fn parameter", "fn f<T: Tr>(x: T) { x.m(); }", None),
        ("fn parameter reference", "fn f<T>(x: &T) { x.m(); }", None),
        (
            "impl parameter",
            "impl<T> W<T> { fn f(x: T) { x.m(); } }",
            None,
        ),
        (
            "ctor on a parameter",
            "fn f<T: Tr>() { let x = T::new(); x.m(); }",
            None,
        ),
        (
            "a path is still a type",
            "fn f<T>(x: a::T) { x.m(); }",
            Some("type:a::T"),
        ),
        ("another name", "fn f<U>(x: T) { x.m(); }", Some("type:T")),
    ]);
}

#[test]
fn the_nearest_binding_before_the_call_wins() {
    assert_hints(&[
        (
            "shadowing let",
            "fn f(x: T) { let x = U::new(); x.m(); }",
            Some("ctor:U::new"),
        ),
        ("clearing let", "fn f(x: T) { let x = g(); x.m(); }", None),
        (
            "let after the call",
            "fn f(x: T) { x.m(); let x = g(); }",
            Some("type:T"),
        ),
        (
            "let in its own value",
            "fn f(x: T) { let x = x.m(); }",
            Some("type:T"),
        ),
        (
            "inner block ends",
            "fn f(x: T) { { let x = g(); } x.m(); }",
            Some("type:T"),
        ),
        (
            "inner block holds the call",
            "fn f(x: T) { { let x: U = g(); x.m(); } }",
            Some("type:U"),
        ),
        (
            "binding in an earlier branch",
            "fn f(x: T) { if c { let x = g(); } x.m(); }",
            Some("type:T"),
        ),
        (
            "else branch of if let",
            "fn f(x: T, o: O) { if let Some(x) = o {} else { x.m(); } }",
            Some("type:T"),
        ),
        (
            "match on another name",
            "fn f(x: T, o: O) { match o { Some(y) => x.m(), _ => {} } }",
            Some("type:T"),
        ),
        (
            "guard binds nothing",
            "fn f(x: T, o: O) { match o { Some(y) if x.m() => {} _ => {} } }",
            Some("type:T"),
        ),
        (
            "closure sees the function",
            "fn f(x: T) { let c = || x.m(); }",
            Some("type:T"),
        ),
        (
            "closure body block",
            "fn f(x: T) { let c = |y: u8| { x.m(); }; }",
            Some("type:T"),
        ),
        (
            "nested fn is its own scope",
            "fn f(x: T) { fn g() { x.m(); } }",
            None,
        ),
        (
            "nested fn parameter",
            "fn f(x: T) { fn g(x: U) { x.m(); } }",
            Some("type:U"),
        ),
        ("unbound", "fn f() { x.m(); }", None),
    ]);
}

#[test]
fn only_a_plain_identifier_receiver_has_a_hint() {
    let extract = RustLanguage::new()
        .expect("query compiles")
        .extract("impl T { fn f(&self, x: T) { self.m(); x.a.m(); x.b().m(); (x).m(); T::m(x); } }")
        .expect("source parses");
    let hints: Vec<_> = extract
        .refs
        .iter()
        .filter(|r| r.last_segment == "m")
        .map(|r| (r.kind, r.receiver_type.clone()))
        .collect();
    assert_eq!(
        hints,
        [
            (RefKind::MethodCall, None),
            (RefKind::MethodCall, None),
            (RefKind::MethodCall, None),
            (RefKind::MethodCall, None),
            (RefKind::Call, None),
        ]
    );
}

#[test]
fn a_method_call_in_a_macro_has_no_hint() {
    let extract = RustLanguage::new()
        .expect("query compiles")
        .extract("fn f(x: T) { println!(\"{}\", x.m()); }")
        .expect("source parses");
    assert!(
        extract
            .refs
            .iter()
            .all(|r| r.receiver_type.is_none() && r.kind != RefKind::MethodCall),
        "{:?}",
        extract.refs
    );
}
