//! Cure (b), a glob import (SPEC-w13 G2a, D9).
//!
//! A bare first segment that nothing in the call's module binds, no item,
//! child module, non-glob `use` or shadowing binding, resolves through the
//! module's `use p::*` uses of workspace modules when exactly one item of
//! that name is found across them. The edge's reason is `use-glob`, at
//! 0.85. A call the base tiers place keeps its edge.

mod support;

use phantom_fractal_lang::{Id, Reason, RefKind, SymbolKind};
use phantom_fractal_lang_rust::{confidence, links};
use support::{Input, edge};

const LIB: &str = "src/lib.rs";

/// A crate whose root declares `mod tests` with `use super::*`, and a
/// function `check` inside it.
fn tests_module() -> (Input, Id) {
    let mut i = Input::new();
    i.module(1, "tests", LIB, "tests");
    i.uses(LIB, "tests", "super", None, true);
    let check = i.symbol(SymbolKind::Function, "check", LIB, "tests");
    (i, check)
}

/// A call `text` from `from` in the `tests` scope of `src/lib.rs`.
fn call_in_tests(i: &mut Input, from: Id, text: &str) -> Id {
    i.call_in(Some(from), LIB, "tests", text, RefKind::Call)
}

#[test]
fn use_glob_has_its_store_text_and_confidence() {
    assert_eq!(Reason::UseGlob.as_str(), "use-glob");
    assert_eq!(confidence(Reason::UseGlob), 0.85);
}

#[test]
fn a_bare_name_only_a_glob_supplies_resolves_by_use_glob() {
    let (mut i, check) = tests_module();
    let helper = i.function("helper");
    let call = call_in_tests(&mut i, check, "helper");
    let through = call_in_tests(&mut i, check, "super::helper");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(call, check, helper, Reason::UseGlob),
            edge(through, check, helper, Reason::PathExact),
        ]
    );
    assert_eq!((counts.edges, counts.unresolved), (2, 0));
}

#[test]
fn a_type_a_glob_supplies_reaches_its_inherent_method() {
    let (mut i, check) = tests_module();
    i.symbol(SymbolKind::Struct, "Gadget", LIB, "");
    let new = i.method("Gadget", None, "new");
    // A trait method is not inherent: no edge, as for a local type.
    i.method("Gadget", Some("Default"), "default");
    let call = call_in_tests(&mut i, check, "Gadget::new");
    call_in_tests(&mut i, check, "Gadget::default");
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(call, check, new, Reason::UseGlob)]);
    assert_eq!(counts.unresolved, 1);
}

#[test]
fn two_candidates_across_the_globbed_modules_give_no_edge() {
    let mut i = Input::new();
    i.module(1, "a", LIB, "a");
    i.module(1, "b", LIB, "b");
    i.symbol(SymbolKind::Function, "twin", LIB, "a");
    i.symbol(SymbolKind::Function, "twin", LIB, "b");
    i.uses(LIB, "", "a", None, true);
    i.uses(LIB, "", "b", None, true);
    let caller = i.function("caller");
    i.call(caller, "twin");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.unresolved, 1);
}

#[test]
fn one_item_reached_by_two_globs_is_one_candidate() {
    let mut i = Input::new();
    let a = i.module(1, "a", LIB, "a");
    let only = i.symbol(SymbolKind::Function, "only", LIB, "a");
    i.uses(LIB, "", "a", None, true);
    i.uses(LIB, "", "crate::a", None, true);
    let caller = i.function("caller");
    let call = i.call(caller, "only");
    let (edges, _) = i.resolve();
    assert_eq!(edges, [edge(call, caller, only, Reason::UseGlob)]);
    let _ = a;
}

#[test]
fn a_shadowing_binding_or_an_explicit_import_keeps_the_call_unresolved() {
    let (mut i, check) = tests_module();
    i.function("helper");
    i.function("other");
    // `let helper = ..` in `check`.
    let shadowed = call_in_tests(&mut i, check, "helper");
    i.input
        .refs
        .iter_mut()
        .find(|call| call.id == shadowed)
        .expect("ref")
        .shadowed = true;
    // `use crate::gone as other;` binds `other` to nothing.
    i.uses(LIB, "tests", "crate::gone", Some("other"), false);
    call_in_tests(&mut i, check, "other");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.unresolved, 2);
}

#[test]
fn a_local_item_is_placed_by_the_base_tier_and_keeps_its_reason() {
    let (mut i, check) = tests_module();
    i.function("helper");
    let local = i.symbol(SymbolKind::Function, "helper", LIB, "tests");
    let call = call_in_tests(&mut i, check, "helper");
    assert_eq!(i.resolve().0, [edge(call, check, local, Reason::Local)]);
}

#[test]
fn a_glob_of_no_workspace_module_supplies_nothing() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Enum, "Shape", LIB, "");
    // `use std::io::*;`, `use Shape::*;` and a glob of a missing module.
    i.uses(LIB, "", "std::io", None, true);
    i.uses(LIB, "", "Shape", None, true);
    i.uses(LIB, "", "crate::gone", None, true);
    let caller = i.function("caller");
    i.call(caller, "read");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.unresolved, 1);
}

#[test]
fn a_re_export_of_the_globbed_module_counts_and_its_own_glob_does_not() {
    let mut i = Input::new();
    i.module(1, "prelude", LIB, "prelude");
    i.module(1, "model", LIB, "model");
    i.module(1, "deep", LIB, "deep");
    let thing = i.symbol(SymbolKind::Function, "thing", LIB, "model");
    i.symbol(SymbolKind::Function, "far", LIB, "deep");
    // `mod prelude { pub use crate::model::thing; pub use crate::deep::*; }`
    i.uses(LIB, "prelude", "crate::model::thing", None, false);
    i.uses(LIB, "prelude", "crate::deep", None, true);
    i.uses(LIB, "", "prelude", None, true);
    let caller = i.function("caller");
    let call = i.call(caller, "thing");
    i.call(caller, "far");
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(call, caller, thing, Reason::UseGlob)]);
    assert_eq!(counts.unresolved, 1);
}

#[test]
fn a_glob_of_a_parent_module_does_not_reach_its_child_modules() {
    let mut i = Input::new();
    i.module(1, "util", LIB, "util");
    i.module(1, "inner", LIB, "inner");
    i.symbol(SymbolKind::Function, "helper", LIB, "util");
    i.uses(LIB, "", "util", None, true);
    // `inner` has no glob of its own.
    let inner_fn = i.symbol(SymbolKind::Function, "run", LIB, "inner");
    i.call_in(Some(inner_fn), LIB, "inner", "helper", RefKind::Call);
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    // Nothing in `inner` binds `helper` and it has no glob: external.
    assert_eq!((counts.unresolved, counts.external), (0, 1));
}

#[test]
fn a_path_in_a_macro_s_tokens_resolves_by_the_same_cure() {
    let (mut i, check) = tests_module();
    let helper = i.function("helper");
    let arg = i.call_in(Some(check), LIB, "tests", "helper", RefKind::MacroArg);
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(arg, check, helper, Reason::UseGlob)]);
    assert_eq!(counts.edges, 1);
}

#[test]
fn the_cure_adds_no_link() {
    let (mut i, check) = tests_module();
    i.function("helper");
    call_in_tests(&mut i, check, "helper");
    assert!(links(&i.input).is_empty());
}
