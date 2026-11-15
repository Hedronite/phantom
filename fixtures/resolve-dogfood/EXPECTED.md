---
title: Expected edges — resolve-dogfood fixture
type: fixture
status: ground-truth
created: "2026-11-14"
updated: "2026-11-14"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w13]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w13]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w13]]"
  - "[[foundry/phantom/goals/GOAL-w13-fence]]"
  - "[[foundry/phantom/w13]]"
tags:
  - phantom
  - fixture
  - w13
---

# Expected edges — `fixtures/resolve-dogfood`

Ground truth for [[foundry/phantom/specs/SPEC-w13]] G2a: the D9 cures, and the cases they leave alone. The test `crates/phantom-fractal-extract/tests/resolve_dogfood.rs` indexes a copy of this package. It asserts three things exactly: the stored edge set equals the **Edges** list, every **Counts** line matches, and the per-reason counts equal the reasons in **Edges**.

There is one package, `dogfood`, with three targets:
- `lib`, rooted at `src/lib.rs`;
- `test-first`, rooted at `tests/first.rs`;
- `test-second`, rooted at `tests/second.rs`.

Both test roots declare `mod common;` on `tests/common/mod.rs`. A symbol is written `<target>:<module path>::<name>`, with the impl owner before a method's name. Each line is `caller -> callee reason`.

## Cases

| Cure | Call | In | Why | Edge |
|---|---|---|---|---|
| (a) | `common::fixture_data()` | `first_case` | the first claimant of `tests/common/mod.rs` | `local` |
| (a) | `common::fixture_data()` | `second_case` | `second` declares `common` too, so it has its own module row on the file, whose items sit with the first | `local` |
| (a) and (b) | `fixture_data()` | `second_case` | `use common::*` globs the later claimant's row, which gives the one `fixture_data` | `use-glob` |
| (b) | `build()` | `entry` | only the glob `use model::*` supplies `build`, and `model` declares one | `use-glob` |
| (b) | `twin()` | `entry` | `model` and `alt` both declare `twin`: two candidates | none (unresolved) |
| (b) | `local_wins()` | `entry` | the crate root declares `local_wins` itself, so the base tier places it and the glob is never tried | `local` to `crate::local_wins` |
| base | `model::build()` | `entry` | a call the base tiers place keeps its edge | `local` |
| (c), skipped | `prelude::Gadget::new()` | `entry` | two `pub use` hops, from `prelude` to `mid` to `model`, which the base tiers already follow | `local` to `model::Gadget::new` |
| (c), skipped | `cycle::looped()` | `entry` | `cycle` re-exports `inner::looped`, which re-exports `super::looped`: a cycle, refused at the depth limit | none (unresolved) |

Cure (c) is skipped by ruling R12, and the reasons are in [[foundry/phantom/evidence/w13/G2-fullstack]]. Its two shapes stay here because the base tiers already decide them.

## Edges

- `lib:crate::entry -> lib:crate::local_wins local`
- `lib:crate::entry -> lib:crate::model::Gadget::new local`
- `lib:crate::entry -> lib:crate::model::build local`
- `lib:crate::entry -> lib:crate::model::build use-glob`
- `test-first:crate::first_case -> test-first:crate::common::fixture_data local`
- `test-second:crate::second_case -> test-first:crate::common::fixture_data local`
- `test-second:crate::second_case -> test-first:crate::common::fixture_data use-glob`

## Counts

- edges: 7
- unresolved: 2
- external: 0
- no_source: 0
- method_calls: 0
- macros: 0
- modules: 11
- orphans: 0

`unresolved` is `twin()` and `cycle::looped()`.

`modules` counts three groups of rows. `lib` has seven: `crate`, `alt`, `cycle`, `cycle::inner`, `mid`, `model` and `prelude`. `test-first` has `crate` and `common`. `test-second` has `crate` and its own `common` row on the shared file.
