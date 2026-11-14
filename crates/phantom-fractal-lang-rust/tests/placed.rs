//! Receiver hints from return types and same-file fields (SPEC-w12 G2a,
//! D6 (a)–(c)), as extracted.

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
fn a_let_bound_to_a_path_call_hints_its_return() {
    assert_hints(&[
        (
            "call",
            "fn f() { let x = make(); x.m(); }",
            Some("ret:make"),
        ),
        (
            "tried",
            "fn f() { let x = make()?; x.m(); }",
            Some("ret?:make"),
        ),
        (
            "turbofish",
            "fn f() { let x = make::<u8>(); x.m(); }",
            Some("ret:make"),
        ),
        (
            "two segments stay ctor",
            "fn f() { let x = a::make(); x.m(); }",
            Some("ctor:a::make"),
        ),
        (
            "a closure the fn binds",
            "fn f() { let make = || 1; let x = make(); x.m(); }",
            None,
        ),
        (
            "a parameter the fn binds",
            "fn f(make: F) { let x = make(); x.m(); }",
            None,
        ),
        ("a method call", "fn f() { let x = o.make(); x.m(); }", None),
        (
            "a called value",
            "fn f() { let x = (make)(); x.m(); }",
            None,
        ),
        (
            "a typed let wins",
            "fn f() { let x: T = make(); x.m(); }",
            Some("type:T"),
        ),
    ]);
}

#[test]
fn a_path_call_receiver_hints_one_level() {
    assert_hints(&[
        ("call", "fn f() { make().m(); }", Some("ret:make")),
        ("tried", "fn f() { make()?.m(); }", Some("ret?:make")),
        ("ctor", "fn f() { T::new().m(); }", Some("ctor:T::new")),
        (
            "tried ctor",
            "fn f() { T::open()?.m(); }",
            Some("ctor?:T::open"),
        ),
        (
            "a closure the fn binds",
            "fn f() { let make = || 1; make().m(); }",
            None,
        ),
        ("a method chain", "fn f() { x.make().m(); }", None),
        ("a chain on a call", "fn f() { make().inner().m(); }", None),
        ("a field chain", "fn f() { a.b.m(); }", None),
        (
            "a generic ctor type",
            "fn f<T: Tr>() { T::new().m(); }",
            None,
        ),
    ]);
}

#[test]
fn a_self_field_of_a_struct_in_this_file_hints_its_type() {
    let with = |item: &str, body: &str| format!("{item}\nimpl S {{ fn g(&self) {{ {body} }} }}");
    assert_hints(&[
        (
            "named field",
            &with("struct S { f: T }", "self.f.m();"),
            Some("type:T"),
        ),
        (
            "reference",
            &with("struct S<'a> { f: &'a T }", "self.f.m();"),
            Some("type:T"),
        ),
        (
            "box",
            &with("struct S { f: Box<T> }", "self.f.m();"),
            Some("type:T"),
        ),
        (
            "scoped type",
            &with("struct S { f: a::T }", "self.f.m();"),
            Some("type:a::T"),
        ),
        (
            "self type",
            &with("struct S { f: Box<Self> }", "self.f.m();"),
            Some("type:S"),
        ),
        (
            "in a closure",
            &with("struct S { f: T }", "let c = || self.f.m();"),
            Some("type:T"),
        ),
        ("tuple field", &with("struct S(T);", "self.0.m();"), None),
        (
            "no such field",
            &with("struct S { f: T }", "self.g.m();"),
            None,
        ),
        (
            "a field of a field",
            &with("struct S { f: T }", "self.f.g.m();"),
            None,
        ),
        (
            "a primitive",
            &with("struct S { f: u32 }", "self.f.m();"),
            None,
        ),
        // One wrapper level comes off, as for a parameter.
        (
            "two wrappers",
            &with("struct S { f: Box<Box<T>> }", "self.f.m();"),
            Some("type:Box"),
        ),
        (
            "another file",
            "use a::S;\nimpl S { fn g(&self) { self.f.m(); } }",
            None,
        ),
        (
            "two structs of that name",
            &with(
                "struct S { f: T }\nmod a { struct S { f: U } }",
                "self.f.m();",
            ),
            None,
        ),
        (
            "a scoped self type",
            "struct S { f: T }\nimpl a::S { fn g(&self) { self.f.m(); } }",
            None,
        ),
        (
            "an enum",
            "enum S { A }\nimpl S { fn g(&self) { self.f.m(); } }",
            None,
        ),
    ]);
}

#[test]
fn a_generic_field_type_is_not_a_type() {
    assert_hints(&[
        (
            "the struct's and the impl's parameter",
            "struct S<T> { f: T }\nimpl<T> S<T> { fn g(&self) { self.f.m(); } }",
            None,
        ),
        (
            "the struct's parameter by another name in the impl",
            "struct S<U> { f: U }\nimpl<V> S<V> { fn g(&self) { self.f.m(); } }",
            None,
        ),
        (
            "a concrete argument of a generic struct",
            "struct S<U> { f: T, g: U }\nimpl<U> S<U> { fn h(&self) { self.f.m(); } }",
            Some("type:T"),
        ),
    ]);
}
