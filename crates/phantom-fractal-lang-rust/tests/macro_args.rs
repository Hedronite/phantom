//! `MacroArg` refs: call-shaped names inside macro token trees
//! (SPEC-w5 G1a), with their receivers, hints and shadowing (SPEC-w12 G1b).

use phantom_fractal_lang::{Caller, Extract, Language, RefKind};
use phantom_fractal_lang_rust::{MAX_HINT_VISITS, RustLanguage};

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

/// `(text, last_segment, line, col)` of every `MacroArg` in `source`.
fn macro_args(source: &str) -> Vec<(String, String, u32, u32)> {
    extract(source)
        .refs
        .into_iter()
        .filter(|r| r.kind == RefKind::MacroArg)
        .map(|r| (r.text, r.last_segment, r.line, r.col))
        .collect()
}

fn texts(source: &str) -> Vec<String> {
    macro_args(source)
        .into_iter()
        .map(|(text, ..)| text)
        .collect()
}

#[test]
fn every_call_shape_is_recorded_with_its_text_and_last_segment() {
    let source = "fn f() {\n    m!(a(1), b::c(2), ::d::e(3), x.m(4), crate::g(), self::h(), \
                  super::i(), Self::new(), j::<u8>(5), Vec::<u8>::new(), \
                  y.collect::<Vec<_>>(), r#try(6), k :: l (7), z . n (8), 1.max(2));\n}\n";
    let got: Vec<(String, String)> = macro_args(source)
        .into_iter()
        .map(|(text, last, ..)| (text, last))
        .collect();
    let want = [
        ("a", "a"),
        ("b::c", "c"),
        ("::d::e", "e"),
        ("m", "m"),
        ("crate::g", "g"),
        ("self::h", "h"),
        ("super::i", "i"),
        ("Self::new", "new"),
        ("j::<u8>", "j"),
        ("Vec::<u8>::new", "new"),
        ("collect::<Vec<_>>", "collect"),
        ("r#try", "r#try"),
        ("k::l", "l"),
        ("n", "n"),
        ("max", "max"),
    ];
    let want: Vec<(String, String)> = want
        .iter()
        .map(|(text, last)| ((*text).to_owned(), (*last).to_owned()))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn positions_follow_the_call_rules() {
    // A path starts at its first token; a method at its name.
    let source = "fn f() {\n    m!(a::b(1), x.y(2));\n    m!(::c(3));\n}\n";
    assert_eq!(
        macro_args(source),
        [
            ("a::b".to_owned(), "b".to_owned(), 2, 8),
            ("y".to_owned(), "y".to_owned(), 2, 19),
            ("::c".to_owned(), "c".to_owned(), 3, 8),
        ]
    );
}

#[test]
fn nested_token_trees_and_nested_macros_count() {
    let source = "fn f() {\n    assert!(vec![f(1)], \"{}\", inner!(g(h(2))), S { x: k(3) });\n}\n";
    assert_eq!(texts(source), ["f", "g", "h", "k"]);
}

#[test]
fn shapes_that_are_not_call_shaped_give_nothing() {
    let source = "fn f() {\n    m!(a, b::c, x.y, z[1], w!(1), v![2], if (c) {}, \
                  match (d) {}, 1 + (2), (e)(3), <T as Tr>::f(4), u . v, \"s(1)\");\n}\n";
    assert_eq!(texts(source), Vec::<String>::new());
}

#[test]
fn macro_rules_bodies_are_not_scanned() {
    let source = "macro_rules! mm { ($e:expr) => { body_call($e) }; }\n\
                  fn f() {\n    outer! { macro_rules! inner_m { () => { hidden() } } visible() }\n}\n";
    assert_eq!(texts(source), ["visible"]);
}

#[test]
fn attributes_are_not_macro_invocations() {
    let source = "#[cfg(test)]\n#[derive(Debug)]\n#[path = \"x.rs\"]\nmod a;\n\
                  #[cfg_attr(test, allow(dead_code))]\nfn f() {}\n";
    assert_eq!(texts(source), Vec::<String>::new());
}

#[test]
fn from_and_scope_follow_the_enclosing_function_and_module() {
    let source = "mod a {\n    fn g() {\n        m!(f());\n    }\n}\n\
                  static T: u8 = m!(top());\n";
    let got = extract(source);
    let args: Vec<_> = got
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MacroArg)
        .collect();
    assert_eq!(args.len(), 2, "{args:?}");
    assert_eq!(args[0].text, "f");
    assert_eq!(args[0].scope, "a");
    assert_eq!(
        args[0].from,
        Some(Caller {
            name: "g".to_owned(),
            owner: None,
            start_line: 2,
        })
    );
    assert_eq!(args[0].receiver, None);
    assert!(!args[0].shadowed);
    assert_eq!(args[1].text, "top");
    assert_eq!(args[1].from, None);
}

#[test]
fn the_macro_itself_is_still_one_macro_ref() {
    let got = extract("fn f() {\n    println!(\"{}\", g());\n}\n");
    let kinds: Vec<(RefKind, &str)> = got.refs.iter().map(|r| (r.kind, r.text.as_str())).collect();
    assert_eq!(
        kinds,
        [(RefKind::Macro, "println"), (RefKind::MacroArg, "g")]
    );
}

/// `(text, receiver, hint, shadowed)` of every `MacroArg` in `source`, in
/// ref order.
type Facts = (String, Option<String>, Option<String>, bool);

fn facts(source: &str) -> Vec<Facts> {
    extract(source)
        .refs
        .into_iter()
        .filter(|r| r.kind == RefKind::MacroArg)
        .map(|r| (r.text, r.receiver, r.receiver_type, r.shadowed))
        .collect()
}

fn fact(text: &str, receiver: Option<&str>, hint: Option<&str>, shadowed: bool) -> Facts {
    (
        text.to_owned(),
        receiver.map(str::to_owned),
        hint.map(str::to_owned),
        shadowed,
    )
}

#[test]
fn a_method_name_keeps_the_tokens_of_its_receiver() {
    let source = "fn f() {\n    m!(self.a(), x.b(), &y.c(), x.y.d(), f(1).e(), z?.g(), \
                  1.h(), (.i()), ::w.j(), k.l::<u8>(), q[0].r(), s . t (2));\n}\n";
    let receivers: Vec<(String, Option<String>)> = facts(source)
        .into_iter()
        .map(|(text, receiver, ..)| (text, receiver))
        .collect();
    let want = [
        ("a", Some("self")),
        ("b", Some("x")),
        ("c", Some("y")),
        ("d", Some("x.y")),
        ("f", None),
        ("e", Some("f(1)")),
        ("g", Some("z?")),
        ("h", Some("1")),
        ("i", Some("")),
        ("j", Some("::w")),
        ("l::<u8>", Some("k")),
        ("r", Some("q[0]")),
        ("t", Some("s")),
    ];
    let want: Vec<(String, Option<String>)> = want
        .iter()
        .map(|(text, receiver)| ((*text).to_owned(), receiver.map(str::to_owned)))
        .collect();
    assert_eq!(receivers, want);
}

#[test]
fn one_identifier_takes_its_hint_from_the_binding_walk() {
    let source = "struct T;\nstruct U;\nimpl T {\n    fn new() -> Self { T }\n    \
                  fn go(&self, x: &U) {\n        let t = T::new();\n        \
                  let u = U {};\n        assert!(self.a() && x.b() && t.c() && u.d());\n        \
                  assert!(x.e.f() && u.g()?.h());\n    }\n}\n\
                  fn plain<V>(v: V, w: Box<U>) {\n    assert!(v.i() && w.j());\n}\n";
    assert_eq!(
        facts(source),
        [
            fact("a", Some("self"), None, false),
            fact("b", Some("x"), Some("type:U"), false),
            fact("c", Some("t"), Some("ctor:T::new"), false),
            fact("d", Some("u"), Some("type:U"), false),
            fact("f", Some("x.e"), None, false),
            fact("g", Some("u"), Some("type:U"), false),
            fact("h", Some("u.g()?"), None, false),
            fact("i", Some("v"), None, false),
            fact("j", Some("w"), Some("type:U"), false),
        ]
    );
}

#[test]
fn a_binding_inside_the_macro_tokens_is_not_seen() {
    // The tokens are never parsed, so only bindings outside them count.
    let source = "fn f(x: T) {\n    m!({ let x = U {}; x.m() });\n}\n";
    assert_eq!(facts(source), [fact("m", Some("x"), Some("type:T"), false)]);
}

#[test]
fn the_hint_walk_in_a_macro_stops_after_4096_visits() {
    assert_eq!(MAX_HINT_VISITS, 4096);
    // The token visits its token tree, the invocation, its statement and
    // block, the n statements before it, then `f` and the parameter.
    let hint = |n: usize| {
        let filler: String = (0..n).map(|i| format!("let a{i} = 0;\n")).collect();
        let source = format!("fn f(x: T) {{\n{filler}m!(x.m());\n}}");
        facts(&source).pop().expect("macro arg").2
    };
    assert_eq!(hint(4090).as_deref(), Some("type:T"));
    assert_eq!(hint(4091), None);
}

#[test]
fn a_single_segment_path_bound_in_the_function_is_shadowed() {
    let source = "fn f(g: u8) {\n    let h = |v: u8| v;\n    \
                  m!(g(1), h(2), k(3), a::h(4), x.h(5), h::<u8>(6));\n}\n\
                  fn h() {}\n";
    assert_eq!(
        facts(source),
        [
            fact("g", None, None, true),
            fact("h", None, None, true),
            fact("k", None, None, false),
            fact("a::h", None, None, false),
            fact("h", Some("x"), None, false),
            fact("h::<u8>", None, None, true),
        ]
    );
}
