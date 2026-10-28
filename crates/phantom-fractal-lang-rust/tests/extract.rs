//! Extraction over the pack fixtures in `fixtures/extract/` (SPEC-extract G2).

use phantom_fractal_lang::{Extract, Import, Item, ItemKind, Language, Visibility};
use phantom_fractal_lang_rust::RustLanguage;

const ITEMS: &str = include_str!("../../../fixtures/extract/items.rs");
const IMPORTS: &str = include_str!("../../../fixtures/extract/imports.rs");

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

fn item(kind: ItemKind, name: &str, visibility: Visibility, start_line: u32) -> Item {
    Item {
        kind,
        name: name.to_owned(),
        visibility,
        start_line,
        is_test: false,
    }
}

fn import(
    path: &str,
    alias: Option<&str>,
    glob: bool,
    visibility: Visibility,
    line: u32,
) -> Import {
    Import {
        path: path.to_owned(),
        alias: alias.map(str::to_owned),
        glob,
        visibility,
        start_line: line,
    }
}

fn restricted(text: &str) -> Visibility {
    Visibility::Restricted(text.to_owned())
}

#[test]
fn items_fixture_gives_kind_name_visibility_and_line() {
    use ItemKind::{Enum, Function, Module, Struct, Trait};
    use Visibility::{Crate, Private, Public};
    let expected = vec![
        item(Function, "open_ledger", Public, 4),
        item(Function, "settle", Crate, 11),
        item(Function, "quiet", Private, 13),
        item(Struct, "Ledger", Public, 19),
        item(Struct, "Marker", Private, 23),
        item(Enum, "Shape", Public, 25),
        item(Enum, "Never", Private, 30),
        item(Trait, "Measure", Public, 32),
        item(Module, "outer", Private, 46),
        item(Struct, "Pair", restricted("pub(super)"), 47),
        item(Trait, "Tally", restricted("pub(in crate::outer)"), 49),
        item(Function, "inner_helper", restricted("pub(self)"), 51),
    ];
    let got = extract(ITEMS);
    assert_eq!(got.items, expected);
    assert!(got.imports.is_empty());
}

#[test]
fn imports_fixture_gives_path_alias_and_glob() {
    use Visibility::{Crate, Private, Public};
    let expected = vec![
        import("super::render::Frame", None, false, Private, 7),
        import("std::collections::HashMap", None, false, Private, 10),
        import("std::io::Result", Some("IoResult"), false, Private, 11),
        import("crate::render::Canvas", None, false, Public, 12),
        import("std::fmt", None, true, Private, 13),
        import("self::cache", Some("store"), false, Crate, 14),
        import("self::cache::Entry", None, false, Crate, 14),
        import("self::cache::nested", None, true, Crate, 14),
        import("::core::mem", None, false, Private, 19),
        import("std::io::Write", Some("_"), false, Private, 20),
    ];
    assert_eq!(extract(IMPORTS).imports, expected);
}

#[test]
fn imports_fixture_gives_modules_as_items() {
    use ItemKind::{Function, Module};
    use Visibility::{Crate, Private, Public};
    let expected = vec![
        item(Module, "parser", Private, 4),
        item(Module, "render", Public, 5),
        item(Module, "cache", Crate, 6),
        item(Function, "scoped", Private, 22),
    ];
    assert_eq!(extract(IMPORTS).items, expected);
}

#[test]
fn syntax_error_keeps_records_outside_the_break() {
    let cases = [
        // The broken header swallows the next item into an ERROR node.
        ("pub struct Before;\nfn broken( {\npub enum After {}\n", 2),
        // The break stays inside a function body.
        (
            "pub struct Before;\nfn broken() { let = ; }\npub enum After {}\n",
            3,
        ),
    ];
    for (source, count) in cases {
        let got = extract(source);
        let names: Vec<&str> = got.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names.len(), count, "{names:?}");
        assert_eq!(names.first(), Some(&"Before"), "{names:?}");
        assert_eq!(names.last(), Some(&"After"), "{names:?}");
    }
}

#[test]
fn language_reports_rust() {
    let rust = RustLanguage::new().expect("query compiles");
    assert_eq!(rust.name(), "rust");
    assert_eq!(rust.extensions(), ["rs"]);
}

#[test]
fn empty_source_gives_empty_extract() {
    assert_eq!(extract(""), Extract::default());
}
