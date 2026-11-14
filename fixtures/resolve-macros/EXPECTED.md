---
title: Expected edges — resolve-macros fixture
type: fixture
status: ground-truth
created: "2026-11-13"
updated: "2026-11-13"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w12]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w12]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w12]]"
  - "[[foundry/phantom/w12]]"
tags:
  - phantom
  - fixture
  - w12
---

# Expected edges — `fixtures/resolve-macros`

Ground truth for [[foundry/phantom/specs/SPEC-w12]] G1b: call-shaped names inside macro arguments resolve by the existing tiers, with nothing expanded. The test `crates/phantom-fractal-extract/tests/resolve_macros.rs` indexes a copy of this workspace. It asserts that the stored edge set equals the **Edges** list exactly, that every **Counts** line matches, and that the per-reason counts equal the reasons in **Edges**. It also asserts the stored `macro_arg` refs listed under **Refs**.

The workspace has two packages. `app` is a lib with the modules `crate`, `crate::helpers` and `crate::shapes`, and names `util` under `[dependencies]`. `util` is a lib.

A symbol is written `<package>/<target>:<module path>::<name>`, with the impl owner before a method's name. Each line is `caller -> callee reason`.

## Cases

| Caller | Inside | Call-shaped name | Why | Edge |
|---|---|---|---|---|
| `local_in_assert` | `assert!` | `local_one(` | declared in the call's module | `local` |
| `use_alias_in_assert_eq` | `assert_eq!` | `double(` | `use helpers::double` | `use-alias` |
| `crate_path_in_vec` | `vec![]` | `util::triple(` | `app` may call `util` by crate name | `crate-path` |
| `path_exact_in_format` | `format!` | `crate::helpers::double(` | starts with `crate` | `path-exact` |
| `assoc_in_a_user_macro` | `check!`, a `macro_rules!` macro | `Ledger::count(` | the one inherent `count` of `Ledger` | `assoc` |
| `Ledger::audit` | `assert!` | `self.total(` | a `self` receiver in an impl method | `self-method` |
| `a_hinted_receiver` | `assert_eq!` | `ledger.total(` | the parameter `ledger: &Ledger` gives `type:Ledger`; `total` is not unique, so only the hint places it | `typed-method` |
| `a_constructed_receiver` | `assert_eq!` | `book.total(` | `let book = Book::new()` gives `ctor:Book::new`, and `new` returns `Self` | `typed-method` |
| `an_unhinted_receiver` | `assert_eq!` | `make().settle(` | the receiver `make()` is not one identifier, so only the name rule applies, and `settle` is the one workspace method of that name | `unique-method` |
| `an_unhinted_receiver` | `assert_eq!` | `make(` | declared in the call's module | `local` |
| `a_stoplisted_name` | `assert_eq!` | `make().len(` | `len` is on `STD_METHOD_NAMES`, though `Ledger::len` is the one workspace `len` | none, not counted |
| `a_stoplisted_name` | `assert_eq!` | `make(` | declared in the call's module | `local` |
| `a_shadowed_name` | `assert!` | `local_one(` | `let local_one = \|\| 2` binds the name in the function | none, not counted |
| `an_external_path` | `assert!` | `std::mem::size_of::<u32>(` | starts with `std` | none, not counted |
| the `check!` body | `macro_rules!` | `hidden_in_body(` | a `macro_rules!` body is never scanned | no ref |

Outside any macro, `make` calls `Ledger::open()` and `a_constructed_receiver` calls `Book::new()`, both by `assoc`. Each macro invocation, `check!` included, is one `macro` ref, counted under `macros` with no edge.

## Edges

- `app/lib:crate::a_constructed_receiver -> app/lib:crate::shapes::Book::new assoc`
- `app/lib:crate::a_constructed_receiver -> app/lib:crate::shapes::Book::total typed-method`
- `app/lib:crate::a_hinted_receiver -> app/lib:crate::shapes::Ledger::total typed-method`
- `app/lib:crate::a_stoplisted_name -> app/lib:crate::make local`
- `app/lib:crate::an_unhinted_receiver -> app/lib:crate::make local`
- `app/lib:crate::an_unhinted_receiver -> app/lib:crate::shapes::Ledger::settle unique-method`
- `app/lib:crate::assoc_in_a_user_macro -> app/lib:crate::shapes::Ledger::count assoc`
- `app/lib:crate::crate_path_in_vec -> util/lib:crate::triple crate-path`
- `app/lib:crate::local_in_assert -> app/lib:crate::local_one local`
- `app/lib:crate::make -> app/lib:crate::shapes::Ledger::open assoc`
- `app/lib:crate::path_exact_in_format -> app/lib:crate::helpers::double path-exact`
- `app/lib:crate::shapes::Ledger::audit -> app/lib:crate::shapes::Ledger::total self-method`
- `app/lib:crate::use_alias_in_assert_eq -> app/lib:crate::helpers::double use-alias`

## Counts

- edges: 13
- unresolved: 0
- external: 0
- no_source: 0
- method_calls: 0
- macros: 12
- modules: 4
- orphans: 0

`macros` is the twelve invocations: `assert!` four times, `assert_eq!` five times, and `vec!`, `format!` and `check!` once each. The three unplaced `macro_arg` refs, `len`, the shadowed `local_one` and `size_of`, are in no count.

## Refs

Every stored `macro_arg` ref, as `caller text receiver hint shadowed`, with `-` for none:

- `a_constructed_receiver total book ctor:Book::new false`
- `a_hinted_receiver total ledger type:Ledger false`
- `a_shadowed_name local_one - - true`
- `a_stoplisted_name len make() - false`
- `a_stoplisted_name make - - false`
- `an_external_path std::mem::size_of::<u32> - - false`
- `an_unhinted_receiver make - - false`
- `an_unhinted_receiver settle make() - false`
- `assoc_in_a_user_macro Ledger::count - - false`
- `audit total self - false`
- `crate_path_in_vec util::triple - - false`
- `local_in_assert local_one - - false`
- `path_exact_in_format crate::helpers::double - - false`
- `use_alias_in_assert_eq double - - false`
