//! `MacroArg` refs: call-shaped names inside macro token trees
//! (SPEC-w5 G1a).

mod support;

use phantom_fractal_lang::{Caller, Extract, Language, RefKind, ResolveCounts};
use phantom_fractal_lang_rust::RustLanguage;
use support::Input;

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

#[test]
fn the_resolver_never_places_or_counts_a_macro_arg() {
    let mut i = Input::new();
    let caller = i.function("caller");
    i.function("helper");
    for text in ["helper", "crate::helper", "std::mem::drop", "x"] {
        i.call_in(Some(caller), "src/lib.rs", "", text, RefKind::MacroArg);
    }
    i.call_in(None, "src/lib.rs", "", "helper", RefKind::MacroArg);
    let (edges, counts) = i.resolve();
    assert_eq!(edges, []);
    assert_eq!(counts, ResolveCounts::default());
}
