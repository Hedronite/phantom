---
title: Expected edges — resolve fixture
type: fixture
status: ground-truth
created: "2026-10-26"
updated: "2026-10-27"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w3]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w3]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w3]]"
  - "[[foundry/phantom/specs/SPEC-w4]]"
tags:
  - phantom
  - fixture
  - w3
---

# Expected edges — `fixtures/resolve`

Ground truth for [[foundry/phantom/specs/SPEC-w3]] G2d. The test
`crates/phantom-fractal-extract/tests/resolve.rs` indexes a copy of this
package and asserts that the stored edge set equals the **Edges** list
exactly and that every **Counts** line matches.

A symbol is written `<target>:<module path>::<name>`, with the impl owner
before a method's name, and `@<line>` after it when another symbol in the
same module has the same owner and name. The targets are `lib` (`src/lib.rs`), `main`
(`src/main.rs`), and `tool` (`src/bin/tool.rs`). Each line is
`caller -> callee reason`.

## Edges

- `lib:crate::entry -> lib:crate::util::helper path-exact`
- `lib:crate::entry -> lib:crate::local_fn path-exact`
- `lib:crate::entry -> lib:crate::local_fn local`
- `lib:crate::entry -> lib:crate::util::helper local`
- `lib:crate::entry -> lib:crate::util::helper use-alias`
- `lib:crate::entry -> lib:crate::util::helper use-alias`
- `lib:crate::entry -> lib:crate::Counter::new@47 assoc`
- `lib:crate::entry -> lib:crate::placed::from_path local`
- `lib:crate::entry -> lib:crate::Counter::new@47 assoc`
- `lib:crate::entry -> lib:crate::Counter::bump unique-method`
- `lib:crate::tests::check -> lib:crate::local_fn path-exact`
- `lib:crate::util::helper -> lib:crate::util::deep::sink local`
- `lib:crate::util::helper -> lib:crate::local_fn path-exact`
- `lib:crate::util::deep::sink -> lib:crate::shapes::area path-exact`
- `lib:crate::shapes::area -> lib:crate::shapes::circle::radius local`
- `lib:crate::placed::from_path -> lib:crate::local_fn path-exact`
- `main:crate::main -> main:crate::cli::run local`
- `main:crate::cli::run -> main:crate::cli::banner local`
- `tool:crate::main -> tool:crate::helper local`
- `lib:crate::tricky::Guest::greet -> lib:crate::tricky::other local`
- `lib:crate::tricky::inner -> lib:crate::tricky::other local`
- `lib:crate::tricky::twin@30 -> lib:crate::tricky::target local`
- `lib:crate::tricky::twin@35 -> lib:crate::tricky::other local`

Lines repeat when two call sites give the same edge: `helper()` and
`aliased()` in `entry`, and the two `Counter::new()` calls. `Counter`
has two `new`s, the inherent one at line 47 and `impl Make`'s at line 59;
`assoc` picks the inherent one.

`src/tricky.rs` holds three shapes where a name and owner alone would
credit a call to the wrong function. Each call is credited to the
function it sits in. The trait default `greet` and the nested `inner`
have no symbol row, so their calls have no source; they never become
edges from `Guest::greet` or the module-level `inner`. The two `#[cfg]`
`twin`s are told apart by their start lines, 30 and 35.

## Cases

- **Unique method (W10):** `Counter::new().bump()` in `entry` reaches `Counter::bump` at 0.5 by `unique-method`: its receiver is a call, so it has no hint, `bump` is defined by one workspace method, and it is not on `STD_METHOD_NAMES`.

## No edge

| Call | In | Why |
|---|---|---|
| `Counter::tally()` | `entry` | Only a trait impl has `tally`; `assoc` needs an inherent method. |
| `std::mem::drop(1)` | `entry` | External: `std`. |
| `area()` | `entry` | Only the glob `use shapes::*` supplies `area`. |
| `twin()` | `entry` | Two `#[cfg]` functions: two candidates. |
| `shadow(2)` | `entry` | Shadowed by `let shadow`. |
| `escaped::nothing()` | `entry` | `#[path = "../../outside.rs"]` leaves the root, so `escaped` has no module. |
| `println!` | `entry` | A macro. |
| `start_value()` | `static START` | Outside any function. |
| `local_fn()` | `tests::check` | Only the glob `use super::*` supplies it. |
| `outside_crate::entry()` | `main` | A crate outside the workspace: external. |
| `found()` | `src/orphan.rs` | No target reaches the file. |
| `target()` | trait default `Greet::greet` | No source: a trait default body has no symbol. |
| `target()` | nested `outer::inner` | No source: a nested fn has no symbol. |
| `inner()` | `tricky::outer` | Shadowed by the nested `fn inner`. |

## Change for SPEC-w4 G2c

`src/main.rs:7` used to call `resolve_fixture::entry()`, this package's own lib by
crate name. Under [[foundry/phantom/specs/SPEC-w4]] G2a a bin may call its own
package's lib, so that call would now get a `crate-path` edge. The line now calls
`outside_crate::entry()`, a crate outside the workspace, so the W3 edges and
counts above are unchanged.

## Counts

- edges: 23
- unresolved: 8
- external: 2
- no_source: 3
- method_calls: 0
- macros: 1
- modules: 11
- orphans: 1
