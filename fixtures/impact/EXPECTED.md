---
title: Expected impact — impact fixture
type: fixture
status: ground-truth
created: "2026-10-28"
updated: "2026-10-28"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w5]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w5]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w5]]"
  - "[[foundry/phantom/w5]]"
options: "max_depth 3, min_confidence 0.2, include_tests true"
tags:
  - phantom
  - fixture
  - w5
---

# Expected impact — `fixtures/impact`

Ground truth for [[foundry/phantom/specs/SPEC-w5]] G2d. The test
`crates/phantom-fractal-analyze/tests/fixture.rs` indexes this workspace
and runs `impact` on every target below at the default options:
`max_depth` 3, `min_confidence` 0.2, `include_tests` true. It asserts
that each target's lines match exactly.

A two-package workspace for `impact`. `engine` is a lib and `app` is a bin that depends on it by `path`.

## Line format

These lines are parsed. Each `## Target NAME` section names one symbol, and its lines are:

- `risk: LEVEL`
- `exact: true|false`
- `reasons: NAME[=COUNT], …`, in the fixed order, or `none`. A reason with no `=COUNT` has a null count.
- `counts: direct N, total N, packages N, tests N, below_floor N`
- `hit: DEPTH PATH:LINE NAME CONFIDENCE REASON SITE_PATH:SITE_LINE`: one per reported non-test hit, sorted by depth, then path, line, and name. `PATH:LINE` is the hit symbol's file and line. `REASON`, `CONFIDENCE`, and the site describe its `via` edge. `CONFIDENCE` is the path product.
- `test: DEPTH PATH:LINE NAME CONFIDENCE REASON SITE_PATH:SITE_LINE`: one per test hit, sorted the same way.

Every group's `total` is its number of `hit:` lines, and no group is truncated.

## Cases

- **A chain to depth 4.** `three` is `root_fn`'s depth 3. `four` would be depth 4, so the default `max_depth` leaves it out.
- **Fan-in.** Five direct callers of `root_fn` use four reasons: `use-alias`, `path-exact`, and `crate-path` from the bin.
- **Products.**
  - `crate-path` (0.95) into the lib from `app::run`.
  - `self-method` (0.9) from `Square::doubled` into `Square::area`.
  - `area` reaches `run` directly at 0.8 by `typed-method`, and `main` at 0.8 × 0.95 = 0.76; the 0.855 path through `doubled` is depth 2, so it is not the one reported.
- **A cycle and a self-call.** `ping` → `pong` → `ping`, and `again` calls itself. Neither reappears.
- **Tests.**
  - `direct_test` has `#[test]`.
  - `inline_tests::helper` sits in an inline `#[cfg(test)]` module.
  - `tests.rs` `helper` and `t_root` sit in a `#[cfg(test)] mod tests;` file module.
  - Tests are listed apart and not expanded, so `inline_tests::outer`, which calls `helper`, is never reached.
- **Macros.** `used_in_macros` appears inside `assert!(…)` and `vec![…]` in `checks`, two `macro_arg` refs with no edge.
- **Typed method (W10).** `square.area()` in `run` reaches `Square::area` at 0.8 by `typed-method`, through `let square = engine::shapes::Square::new(3);`, whose `new` returns `Self`.
- **Targets.**
  - `Square::area` is a trait-impl method.
  - `root_fn` is a `pub` lib function.
  - `hidden` and `report` are private, and `report` is in the bin.

## Target root_fn

- risk: medium
- exact: false
- reasons: external_users_possible
- counts: direct 5, total 8, packages 2, tests 2, below_floor 0
- hit: 1 app/src/main.rs:9 run 0.95 crate-path app/src/main.rs:13
- hit: 1 app/src/main.rs:16 report 0.95 crate-path app/src/main.rs:17
- hit: 1 engine/src/chain.rs:5 one 0.95 use-alias engine/src/chain.rs:6
- hit: 1 engine/src/chain.rs:21 fan_a 1.0 path-exact engine/src/chain.rs:22
- hit: 1 engine/src/chain.rs:25 fan_b 1.0 path-exact engine/src/chain.rs:26
- hit: 2 app/src/main.rs:5 main 0.9025 local app/src/main.rs:6
- hit: 2 engine/src/chain.rs:9 two 0.9025 local engine/src/chain.rs:10
- hit: 3 engine/src/chain.rs:13 three 0.857375 local engine/src/chain.rs:14
- test: 1 engine/src/tests.rs:3 helper 1.0 path-exact engine/src/tests.rs:4
- test: 1 engine/src/tests.rs:8 t_root 1.0 path-exact engine/src/tests.rs:9

## Target hidden

- risk: low
- exact: true
- reasons: none
- counts: direct 1, total 1, packages 1, tests 2, below_floor 0
- hit: 1 engine/src/lib.rs:17 uses_hidden 0.95 local engine/src/lib.rs:18
- test: 1 engine/src/lib.rs:22 direct_test 0.95 local engine/src/lib.rs:23
- test: 1 engine/src/lib.rs:28 helper 1.0 path-exact engine/src/lib.rs:29

## Target used_in_macros

- risk: unknown
- exact: false
- reasons: macro_boundary=2, external_users_possible
- counts: direct 1, total 1, packages 1, tests 0, below_floor 0
- hit: 1 engine/src/lib.rs:41 real 0.95 local engine/src/lib.rs:42

## Target ping

- risk: low
- exact: false
- reasons: external_users_possible
- counts: direct 2, total 2, packages 1, tests 0, below_floor 0
- hit: 1 engine/src/ring.rs:10 pong 0.95 local engine/src/ring.rs:11
- hit: 1 engine/src/ring.rs:14 again 0.95 local engine/src/ring.rs:18

## Target area

- risk: low
- exact: false
- reasons: trait_dispatch
- counts: direct 2, total 3, packages 2, tests 0, below_floor 0
- hit: 1 app/src/main.rs:9 run 0.8 typed-method app/src/main.rs:12
- hit: 1 engine/src/shapes.rs:20 doubled 0.9 self-method engine/src/shapes.rs:21
- hit: 2 app/src/main.rs:5 main 0.76 local app/src/main.rs:6

## Target doubled

- risk: low
- exact: false
- reasons: external_users_possible
- counts: direct 1, total 2, packages 1, tests 0, below_floor 0
- hit: 1 app/src/main.rs:9 run 0.95 crate-path app/src/main.rs:11
- hit: 2 app/src/main.rs:5 main 0.9025 local app/src/main.rs:6

## Target report

- risk: low
- exact: true
- reasons: none
- counts: direct 1, total 2, packages 1, tests 0, below_floor 0
- hit: 1 app/src/main.rs:9 run 0.95 local app/src/main.rs:13
- hit: 2 app/src/main.rs:5 main 0.9025 local app/src/main.rs:6
