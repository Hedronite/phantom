//! `is_test` on items and methods, and `cfg_test` on `mod` declarations
//! (SPEC-w5 G1a).

use phantom_fractal_lang::{Extract, Language};
use phantom_fractal_lang_rust::RustLanguage;

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

/// The names of the items in `source` marked as tests, and the rest.
fn items(source: &str) -> (Vec<String>, Vec<String>) {
    let (tests, rest): (Vec<_>, Vec<_>) = extract(source)
        .items
        .into_iter()
        .partition(|item| item.is_test);
    let names = |items: Vec<phantom_fractal_lang::Item>| -> Vec<String> {
        items.into_iter().map(|item| item.name).collect()
    };
    (names(tests), names(rest))
}

#[test]
fn test_attributes_mark_an_item() {
    let source = "#[test]\nfn a() {}\n\
                  #[tokio::test]\nasync fn b() {}\n\
                  #[tokio::test(flavor = \"multi_thread\")]\nasync fn c() {}\n\
                  #[core::prelude::v1::test]\nfn d() {}\n\
                  /// doc\n#[test]\n// note\n#[ignore]\nfn e() {}\n\
                  #[testing]\nfn f() {}\n\
                  #[test_case(1)]\nfn g() {}\n\
                  #[should_panic]\nfn h() {}\n\
                  fn plain() {}\n";
    let (tests, rest) = items(source);
    assert_eq!(tests, ["a", "b", "c", "d", "e"]);
    assert_eq!(rest, ["f", "g", "h", "plain"]);
}

#[test]
fn only_cfg_test_exactly_marks_an_item() {
    let source = "#[cfg(test)]\nfn a() {}\n\
                  #[cfg( test )]\nstruct B;\n\
                  #[cfg(test)]\nenum C {}\n\
                  #[cfg(test)]\ntrait D {}\n\
                  #[cfg(all(test, unix))]\nfn e() {}\n\
                  #[cfg(not(test))]\nfn f() {}\n\
                  #[cfg(any(test))]\nfn g() {}\n\
                  #[cfg_attr(test, allow(dead_code))]\nfn h() {}\n";
    let (tests, rest) = items(source);
    assert_eq!(tests, ["a", "B", "C", "D"]);
    assert_eq!(rest, ["e", "f", "g", "h"]);
}

#[test]
fn everything_inside_an_inline_cfg_test_module_is_a_test() {
    let source = "#[cfg(test)]\nmod tests {\n    fn a() {}\n    struct S;\n    \
                  impl S { fn m(&self) {} }\n    mod deep {\n        fn b() {}\n    }\n}\n\
                  #[cfg(all(test))]\nmod near {\n    fn c() {}\n}\n\
                  mod plain {\n    #[test]\n    fn d() {}\n    fn e() {}\n}\n";
    let got = extract(source);
    let (tests, rest) = items(source);
    assert_eq!(tests, ["tests", "a", "S", "deep", "b", "d"]);
    assert_eq!(rest, ["near", "c", "plain", "e"]);
    let methods: Vec<(&str, bool)> = got
        .methods
        .iter()
        .map(|m| (m.name.as_str(), m.is_test))
        .collect();
    assert_eq!(methods, [("m", true)]);
}

#[test]
fn methods_are_marked_by_their_own_attributes() {
    // An `impl` block's own `#[cfg(test)]` is not one of the counted forms.
    let source = "struct S;\nimpl S {\n    #[test]\n    fn a() {}\n    #[cfg(test)]\n    \
                  fn b() {}\n    fn c() {}\n}\n#[cfg(test)]\nimpl S {\n    fn d() {}\n}\n";
    let methods: Vec<(String, bool)> = extract(source)
        .methods
        .into_iter()
        .map(|m| (m.name, m.is_test))
        .collect();
    let want = [("a", true), ("b", true), ("c", false), ("d", false)];
    let want: Vec<(String, bool)> = want
        .iter()
        .map(|(name, test)| ((*name).to_owned(), *test))
        .collect();
    assert_eq!(methods, want);
}

#[test]
fn a_mod_declaration_records_cfg_test_exactly() {
    let source = "#[cfg(test)]\nmod tests;\nmod a;\n#[cfg(all(test, unix))]\nmod b;\n\
                  #[cfg(test)]\n#[path = \"other.rs\"]\nmod c;\n\
                  #[cfg(test)]\nmod d {\n    mod e;\n}\n";
    let decls: Vec<(String, bool, Option<String>)> = extract(source)
        .mod_decls
        .into_iter()
        .map(|decl| (decl.name, decl.cfg_test, decl.path))
        .collect();
    assert_eq!(
        decls,
        [
            ("tests".to_owned(), true, None),
            ("a".to_owned(), false, None),
            ("b".to_owned(), false, None),
            ("c".to_owned(), true, Some("other.rs".to_owned())),
            ("d".to_owned(), true, None),
            // Below a test module, but its own declaration is not marked;
            // the crate graph carries the module down.
            ("e".to_owned(), false, None),
        ]
    );
}
