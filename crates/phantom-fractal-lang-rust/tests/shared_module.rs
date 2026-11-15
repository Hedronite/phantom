//! Cure (a), a module file that several roots declare (SPEC-w13 G2a, D9).
//!
//! Each later claimant of the file has a module row of its own on it,
//! while the file's items and `use` bindings sit with its first module.
//! A lookup in any of those rows answers from the first, so the file's
//! items resolve from every claiming root by the existing tiers and
//! reasons.

mod support;

use phantom_fractal_lang::{Id, LinkKind, Node, Reason, RefKind, SymbolKind, TargetKind};
use phantom_fractal_lang_rust::links;
use support::{Input, edge};

const COMMON: &str = "tests/common/mod.rs";

/// Two test targets, `a` and `b`, each declaring `mod common;` on one
/// file whose items are `fresh` and `start`: the first module row of the
/// file is `a`'s.
struct Shared {
    i: Input,
    a: Id,
    b: Id,
    fresh: Id,
}

fn shared() -> Shared {
    let mut i = Input::new();
    let a = i.target("pkg", TargetKind::Test, "tests/a.rs");
    let b = i.target("pkg", TargetKind::Test, "tests/b.rs");
    i.module(a, "common", COMMON, "");
    i.module(b, "common", COMMON, "");
    let fresh = i.symbol(SymbolKind::Function, "fresh", COMMON, "");
    i.symbol(SymbolKind::Function, "start", COMMON, "");
    Shared { i, a, b, fresh }
}

/// A function `name` at the root of the test target rooted at `root`.
fn test_fn(i: &mut Input, root: &str, name: &str) -> Id {
    i.symbol(SymbolKind::Function, name, root, "")
}

#[test]
fn each_claiming_root_reaches_the_file_items_by_the_existing_tiers() {
    let mut s = shared();
    let in_a = test_fn(&mut s.i, "tests/a.rs", "in_a");
    let in_b = test_fn(&mut s.i, "tests/b.rs", "in_b");
    let first = s.i.call_from(in_a, "tests/a.rs", "common::fresh");
    let later = s.i.call_from(in_b, "tests/b.rs", "common::fresh");
    let by_path = s.i.call_from(in_b, "tests/b.rs", "crate::common::fresh");
    let (edges, counts) = s.i.resolve();
    assert_eq!(
        edges,
        [
            edge(first, in_a, s.fresh, Reason::Local),
            edge(later, in_b, s.fresh, Reason::Local),
            edge(by_path, in_b, s.fresh, Reason::PathExact),
        ]
    );
    assert_eq!(counts.unresolved, 0);
    let _ = (s.a, s.b);
}

#[test]
fn a_later_claimant_reaches_the_file_items_through_a_use_and_a_glob() {
    let mut s = shared();
    s.i.uses("tests/b.rs", "", "common::fresh", None, false);
    let in_b = test_fn(&mut s.i, "tests/b.rs", "in_b");
    let alias = s.i.call_from(in_b, "tests/b.rs", "fresh");
    let mut g = shared();
    g.i.uses("tests/b.rs", "", "common", None, true);
    let in_g = test_fn(&mut g.i, "tests/b.rs", "in_b");
    let glob = g.i.call_from(in_g, "tests/b.rs", "fresh");
    assert_eq!(
        s.i.resolve().0,
        [edge(alias, in_b, s.fresh, Reason::UseAlias)]
    );
    assert_eq!(
        g.i.resolve().0,
        [edge(glob, in_g, g.fresh, Reason::UseGlob)]
    );
}

#[test]
fn a_call_inside_the_shared_file_resolves_from_its_first_module() {
    let mut s = shared();
    let start =
        s.i.input
            .symbols
            .iter()
            .find(|symbol| symbol.name == "start")
            .map(|symbol| symbol.id)
            .expect("start");
    let call =
        s.i.call_in(Some(s.fresh), COMMON, "", "start", RefKind::Call);
    // `super` from the file's first module is `a`'s root.
    let root_fn = test_fn(&mut s.i, "tests/a.rs", "helper");
    let up =
        s.i.call_in(Some(s.fresh), COMMON, "", "super::helper", RefKind::Call);
    assert_eq!(
        s.i.resolve().0,
        [
            edge(call, s.fresh, start, Reason::Local),
            edge(up, s.fresh, root_fn, Reason::PathExact),
        ]
    );
}

#[test]
fn a_root_that_does_not_declare_the_file_does_not_reach_it() {
    let mut s = shared();
    let c = s.i.target("pkg", TargetKind::Test, "tests/c.rs");
    let in_c = test_fn(&mut s.i, "tests/c.rs", "in_c");
    s.i.call_from(in_c, "tests/c.rs", "common::fresh");
    let (edges, counts) = s.i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    // Nothing in `c` binds `common`, and it has no glob: external.
    assert_eq!((counts.unresolved, counts.external), (0, 1));
    let _ = c;
}

#[test]
fn an_import_of_the_shared_module_keeps_the_link_it_had() {
    let mut s = shared();
    // Each root declares `mod common;`, an item of kind module.
    s.i.symbol(SymbolKind::Module, "common", "tests/a.rs", "");
    let item_b = s.i.symbol(SymbolKind::Module, "common", "tests/b.rs", "");
    s.i.uses("tests/a.rs", "", "crate::common", None, false);
    s.i.uses("tests/b.rs", "", "crate::common", None, false);
    let first_row =
        s.i.input
            .modules
            .iter()
            .find(|module| module.file == COMMON)
            .map(|module| module.id)
            .expect("first row");
    let targets: Vec<Node> = links(&s.i.input)
        .into_iter()
        .filter(|link| link.kind == LinkKind::Import)
        .map(|link| link.target)
        .collect();
    // The first claimant's link names its module row; the later claimant's
    // names its `mod common` item, as before its row existed.
    assert_eq!(targets, [Node::Module(first_row), Node::Symbol(item_b)]);
    let _ = (s.a, s.b);
}
