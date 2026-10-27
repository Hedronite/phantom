---
title: Expected edges — resolve fixture
type: fixture
status: ground-truth
created: "2026-10-26"
updated: "2026-10-26"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w3]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w3]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w3]]"
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
before a method's name. The targets are `lib` (`src/lib.rs`), `main`
(`src/main.rs`), and `tool` (`src/bin/tool.rs`). Each line is
`caller -> callee reason`.

## Edges

- `lib:crate::entry -> lib:crate::util::helper path-exact`
- `lib:crate::entry -> lib:crate::local_fn path-exact`
- `lib:crate::entry -> lib:crate::local_fn local`
- `lib:crate::entry -> lib:crate::util::helper local`
- `lib:crate::entry -> lib:crate::util::helper use-alias`
- `lib:crate::entry -> lib:crate::util::helper use-alias`
- `lib:crate::entry -> lib:crate::Counter::new assoc`
- `lib:crate::entry -> lib:crate::placed::from_path local`
- `lib:crate::entry -> lib:crate::Counter::new assoc`
- `lib:crate::tests::check -> lib:crate::local_fn path-exact`
- `lib:crate::util::helper -> lib:crate::util::deep::sink local`
- `lib:crate::util::helper -> lib:crate::local_fn path-exact`
- `lib:crate::util::deep::sink -> lib:crate::shapes::area path-exact`
- `lib:crate::shapes::area -> lib:crate::shapes::circle::radius local`
- `lib:crate::placed::from_path -> lib:crate::local_fn path-exact`
- `main:crate::main -> main:crate::cli::run local`
- `main:crate::cli::run -> main:crate::cli::banner local`
- `tool:crate::main -> tool:crate::helper local`

Lines repeat when two call sites give the same edge: `helper()` and
`aliased()` in `entry`, and the two `Counter::new()` calls.

## No edge

| Call | In | Why |
|---|---|---|
| `Counter::tally()` | `entry` | Only a trait impl has `tally`; `assoc` needs an inherent method. |
| `std::mem::drop(1)` | `entry` | External: `std`. |
| `area()` | `entry` | Only the glob `use shapes::*` supplies `area`. |
| `twin()` | `entry` | Two `#[cfg]` functions: two candidates. |
| `shadow(2)` | `entry` | Shadowed by `let shadow`. |
| `escaped::nothing()` | `entry` | `#[path = "../../outside.rs"]` leaves the root, so `escaped` has no module. |
| `Counter::new().bump()` | `entry` | A method call on a value. |
| `println!` | `entry` | A macro. |
| `start_value()` | `static START` | Outside any function. |
| `local_fn()` | `tests::check` | Only the glob `use super::*` supplies it. |
| `resolve_fixture::entry()` | `main` | Another crate by name: external. |
| `found()` | `src/orphan.rs` | No target reaches the file. |

## Counts

- edges: 18
- unresolved: 7
- external: 2
- no_source: 1
- method_calls: 1
- macros: 1
- modules: 10
- orphans: 1
