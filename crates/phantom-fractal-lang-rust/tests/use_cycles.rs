//! `use` bindings that name themselves or each other (SPEC-w13 G1b).
//!
//! A lookup follows `use` chains up to 8 deep, and gives up there as
//! unresolved. Before a lookup kept its answers, k bindings of one name
//! that each name that name cost about k^8 walks, and a fuzzed file took
//! minutes. Each shape below resolves exactly as before. The `_in_time`
//! cases reach counts that took minutes before; a regression shows as the
//! test runner's time limit.

mod support;

use phantom_fractal_lang::{Language, Reason, SymbolKind};
use phantom_fractal_lang_rust::{RustLanguage, links};
use support::{Input, edge};

/// `k` copies of `use gpui;`, 75 leaves `use gpui::A<i>;`, and a call
/// `gpui::g()` from `caller`.
fn self_named(k: usize) -> Input {
    let mut i = Input::new();
    for _ in 0..k {
        i.uses("src/lib.rs", "", "gpui", None, false);
    }
    for leaf in 0..75 {
        i.uses("src/lib.rs", "", &format!("gpui::A{leaf}"), None, false);
    }
    let caller = i.function("caller");
    i.call(caller, "gpui::g");
    i
}

/// The one call is unresolved and no `use` gives a link.
fn assert_unresolved(i: &Input, case: &str) {
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{case}: {edges:?}");
    assert_eq!((counts.unresolved, counts.external), (1, 0), "{case}");
    assert!(links(&i.input).is_empty(), "{case}");
}

#[test]
fn a_use_that_names_itself_leaves_its_paths_unresolved() {
    for k in 1..=3 {
        assert_unresolved(&self_named(k), &format!("k={k}"));
    }
}

#[test]
fn many_uses_that_name_themselves_resolve_in_time() {
    // 12^8 walks without kept answers.
    assert_unresolved(&self_named(12), "k=12");
}

#[test]
fn a_self_leaf_is_the_same_binding_as_a_plain_use() {
    let extract = RustLanguage::new()
        .expect("the query compiles")
        .extract("use gpui::{self};\nuse gpui;\n")
        .expect("the source parses");
    let paths: Vec<(&str, Option<&str>)> = extract
        .imports
        .iter()
        .map(|import| (import.path.as_str(), import.alias.as_deref()))
        .collect();
    assert_eq!(paths, [("gpui", None), ("gpui", None)]);
}

/// `use b as a; use a as b;`, `k` times over, and a call `a::f()`.
fn aliased_pair(k: usize) -> Input {
    let mut i = Input::new();
    for _ in 0..k {
        i.uses("src/lib.rs", "", "b", Some("a"), false);
        i.uses("src/lib.rs", "", "a", Some("b"), false);
    }
    let caller = i.function("caller");
    i.call(caller, "a::f");
    i
}

#[test]
fn two_aliases_that_name_each_other_leave_the_path_unresolved() {
    assert_unresolved(&aliased_pair(1), "k=1");
    assert_unresolved(&aliased_pair(2), "k=2");
}

#[test]
fn many_aliases_that_name_each_other_resolve_in_time() {
    assert_unresolved(&aliased_pair(8), "k=8");
}

/// Modules `m` and `n` that each re-export `f` from the other, `k` times
/// over, and a call `m::f()` from the crate root.
fn module_cycle(k: usize) -> Input {
    let mut i = Input::new();
    i.module(1, "m", "src/lib.rs", "m");
    i.module(1, "n", "src/lib.rs", "n");
    for _ in 0..k {
        i.uses("src/lib.rs", "m", "super::n::f", None, false);
        i.uses("src/lib.rs", "n", "super::m::f", None, false);
    }
    let caller = i.function("caller");
    i.call(caller, "m::f");
    i
}

#[test]
fn modules_that_re_export_from_each_other_leave_the_path_unresolved() {
    assert_unresolved(&module_cycle(1), "k=1");
    assert_unresolved(&module_cycle(2), "k=2");
}

#[test]
fn many_re_exports_between_two_modules_resolve_in_time() {
    assert_unresolved(&module_cycle(8), "k=8");
}

/// `k` copies of `use x;` beside `use crate::inner as x;`, where `inner`
/// declares `f`, and a call `x::f()`.
fn cycle_beside_a_real_binding(k: usize) -> (Input, [i64; 3]) {
    let mut i = Input::new();
    i.module(1, "inner", "src/lib.rs", "inner");
    let f = i.symbol(SymbolKind::Function, "f", "src/lib.rs", "inner");
    for _ in 0..k {
        i.uses("src/lib.rs", "", "x", None, false);
    }
    i.uses("src/lib.rs", "", "crate::inner", Some("x"), false);
    let caller = i.function("caller");
    let call = i.call(caller, "x::f");
    (i, [call, caller, f])
}

#[test]
fn a_cycle_beside_a_real_binding_keeps_the_real_edge() {
    for k in [1, 2, 12] {
        let (i, [call, caller, f]) = cycle_beside_a_real_binding(k);
        let (edges, counts) = i.resolve();
        assert_eq!(edges, [edge(call, caller, f, Reason::UseAlias)], "k={k}");
        assert_eq!(counts.edges, 1, "k={k}");
    }
}
