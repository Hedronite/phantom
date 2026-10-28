//! Scopes, methods, and module declarations over
//! `fixtures/extract/scopes.rs` (SPEC-w3 G0b).

use phantom_fractal_lang::{Extract, Language, Method, ModDecl, Visibility};
use phantom_fractal_lang_rust::RustLanguage;

const SCOPES: &str = include_str!("../../../fixtures/extract/scopes.rs");
const FIXTURES: [&str; 4] = [
    SCOPES,
    include_str!("../../../fixtures/extract/items.rs"),
    include_str!("../../../fixtures/extract/imports.rs"),
    include_str!("../../../fixtures/extract/refs.rs"),
];

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

#[test]
fn item_scopes_follow_inline_modules() {
    let got = extract(SCOPES);
    let named: Vec<(&str, &str)> = got
        .items
        .iter()
        .zip(&got.item_scopes)
        .map(|(item, scope)| (item.name.as_str(), scope.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("store", ""),
            ("placed", ""),
            ("Ledger", ""),
            ("outer", ""),
            ("make", "outer"),
            ("inner", "outer"),
            ("deep", "outer::inner"),
            ("tests", ""),
            ("builds", "tests"),
        ]
    );
}

#[test]
fn import_scopes_follow_inline_modules() {
    let got = extract(SCOPES);
    let named: Vec<(&str, &str)> = got
        .imports
        .iter()
        .zip(&got.import_scopes)
        .map(|(import, scope)| (import.path.as_str(), scope.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("std::fmt", ""),
            ("super::Ledger", "outer"),
            ("super", "tests")
        ]
    );
}

#[test]
fn scopes_are_parallel_to_items_and_imports_in_every_fixture() {
    for source in FIXTURES {
        let got = extract(source);
        assert_eq!(got.item_scopes.len(), got.items.len());
        assert_eq!(got.import_scopes.len(), got.imports.len());
    }
    let grouped = extract("mod m { use a::{b, c::{d, e as f}, g::*}; }");
    assert_eq!(grouped.imports.len(), 4);
    assert_eq!(grouped.import_scopes, ["m", "m", "m", "m"]);
}

#[test]
fn the_new_fields_leave_items_and_imports_unchanged() {
    let got = extract(include_str!("../../../fixtures/extract/imports.rs"));
    let paths: Vec<&str> = got
        .imports
        .iter()
        .map(|import| import.path.as_str())
        .collect();
    assert_eq!(paths.len(), 10);
    assert_eq!(paths.first(), Some(&"super::render::Frame"));
    assert_eq!(got.import_scopes.first().map(String::as_str), Some("cache"));
}

fn method(
    owner_path: &str,
    trait_name: Option<&str>,
    name: &str,
    visibility: Visibility,
    scope: &str,
    start_line: u32,
) -> Method {
    Method {
        owner: "Ledger".to_owned(),
        owner_path: owner_path.to_owned(),
        trait_name: trait_name.map(str::to_owned),
        name: name.to_owned(),
        visibility,
        scope: scope.to_owned(),
        start_line,
        is_test: false,
    }
}

#[test]
fn methods_carry_owner_trait_and_scope() {
    use Visibility::{Private, Public};
    let expected = vec![
        method("Ledger<T>", None, "new", Public, "", 13),
        method("Ledger<T>", None, "peek", Private, "", 17),
        method(
            "crate::Ledger<T>",
            Some("fmt::Debug"),
            "fmt",
            Private,
            "",
            23,
        ),
        method(
            "super::super::Ledger<u8>",
            None,
            "byte",
            Public,
            "outer::inner",
            37,
        ),
    ];
    assert_eq!(extract(SCOPES).methods, expected);
}

#[test]
fn methods_skip_trait_bodies_and_function_local_impls() {
    let got = extract("trait T { fn d() {} }\nfn f() { impl S { fn m() {} } }\n");
    assert!(got.methods.is_empty(), "{:?}", got.methods);
}

#[test]
fn owner_strips_references_and_keeps_other_types_as_written() {
    let got =
        extract("impl<'a> T for &'a mut a::B<'a> { fn x() {} }\nimpl T for u8 { fn y() {} }\n");
    let owners: Vec<&str> = got.methods.iter().map(|m| m.owner.as_str()).collect();
    assert_eq!(owners, ["B", "u8"]);
}

fn module(name: &str, scope: &str, inline: bool, path: Option<&str>, line: u32) -> ModDecl {
    ModDecl {
        name: name.to_owned(),
        scope: scope.to_owned(),
        inline,
        path: path.map(str::to_owned),
        line,
        cfg_test: false,
    }
}

#[test]
fn mod_decls_carry_inline_and_path() {
    let expected = vec![
        module("store", "", false, None, 6),
        module("placed", "", false, Some("other/place.rs"), 8),
        module("outer", "", true, None, 28),
        module("inner", "outer", true, None, 33),
        ModDecl {
            cfg_test: true,
            ..module("tests", "", true, None, 43)
        },
    ];
    assert_eq!(extract(SCOPES).mod_decls, expected);
}

#[test]
fn path_attribute_is_found_past_other_attributes_and_comments() {
    let source =
        "#[path = \"x/y.rs\"]\n// note\n#[cfg(unix)]\nmod a;\n#[path = r\"z.rs\"] mod b;\n";
    let got = extract(source);
    let paths: Vec<Option<&str>> = got
        .mod_decls
        .iter()
        .map(|decl| decl.path.as_deref())
        .collect();
    assert_eq!(paths, [Some("x/y.rs"), Some("z.rs")]);
}

#[test]
fn a_call_in_mod_tests_has_the_tests_scope() {
    let got = extract(SCOPES);
    let make_one = got
        .refs
        .iter()
        .find(|r| r.text == "make_one")
        .expect("make_one ref");
    assert_eq!(make_one.scope, "tests");
    assert_eq!(
        make_one.from.as_ref().map(|from| from.name.as_str()),
        Some("builds")
    );
}
