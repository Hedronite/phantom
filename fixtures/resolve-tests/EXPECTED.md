---
title: Expected edges — resolve-tests fixture
type: fixture
status: ground-truth
created: "2026-10-31"
updated: "2026-10-31"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w11]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w11]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w11]]"
tags:
  - phantom
  - fixture
  - w11
---

# Expected edges — `fixtures/resolve-tests`

Ground truth for [[foundry/phantom/specs/SPEC-w11]] G1b: integration tests as `test` targets. The test `crates/phantom-fractal-extract/tests/resolve_tests.rs` indexes a copy of this workspace. It asserts that the stored edge set equals the **Edges** list exactly, that every **Counts** line matches, and that the per-reason counts equal the reasons in **Edges**.

The workspace has two packages. `app` has a lib, a bin, two test targets, a bench and an example. It names `probe` only under `[dev-dependencies]`, as `probe.workspace = true`, through the root's `[workspace.dependencies]`. `probe` is a lib.

A symbol is written `<package>/<target>:<module path>::<name>`, with the impl owner before a method's name. The target is the root file's stem for a lib or bin (`lib`, `main`), and `test-<name>` for a test target: `test-plain` is `app/tests/plain.rs` and `test-suite` is `app/tests/suite/main.rs`. Each line is `caller -> callee reason`.

## Targets

| Target | Kind | Root | Modules |
|---|---|---|---|
| `app` | lib | `app/src/lib.rs` | `crate` |
| `app` | bin | `app/src/main.rs` | `crate` |
| `probe` | lib | `probe/src/lib.rs` | `crate` |
| `plain` | test | `app/tests/plain.rs` | `crate`, `crate::common` (`app/tests/common/mod.rs`) |
| `suite` | test | `app/tests/suite/main.rs` | `crate` |

- **`tests/common/mod.rs` is reached from two roots.**
  - `plain` reaches it with `mod common;`, looking beside itself.
  - `suite` reaches it with `#[path = "../common/mod.rs"] mod common;`.
  - A file keeps its first module, so it is `crate::common` of `plain` only, and `suite` has no `common` module.
- **Every module of a test target is a test module**, so every symbol in the three test files is a test symbol.
- **Not targets in W11:** `app/benches/speed.rs` and `app/examples/demo.rs`. They stay orphans.

## Cases

| Caller | Call | Why | Edge |
|---|---|---|---|
| `common::fresh` | `start()` | a function of the same module | `local` |
| `common::start` | `Counter::new()` | `use app::Counter` names the lib's type | `crate-path` |
| `own_lib_by_crate_path` | `app::add(1, 2)` | a test target may call its own package's lib | `crate-path` |
| `dev_dependency_by_crate_path` | `probe::check(1)` | `probe` is a dev-dependency of `app`, and the caller is in a test target | `crate-path` |
| `through_a_use_alias` | `fresh_counter()` | `use common::fresh as fresh_counter` | `use-alias` |
| `through_a_use_alias` | `plus(2, 2)` | `use app::add as plus` names the lib | `crate-path` |
| `into_common` | `common::fresh()` | `common` is a child module of `plain` | `local` |
| `a_typed_method_call` | `Counter::new()` | as above | `crate-path` |
| `a_typed_method_call` | `counter.bump()` | `let mut counter = Counter::new()` gives `ctor:Counter::new`; placing it needs the test module's `use app::Counter` | `typed-method` |
| `suite_case` | `app::add(3, 4)` | the second test target calls the lib too | `crate-path` |
| `suite_case` | `common::fresh()` | `suite` has no `common` module | none (unresolved) |
| `app` lib `checked` | `probe::check(value)` | a lib ignores dev entries | none (external) |
| `app` bin `main` | `app::add(1, 2)` | a bin may call its own lib, as before | `crate-path` |
| `app` bin `main` | `probe::check(total)` | a bin ignores dev entries | none (external) |
| bench and example `main` | `app::add(…)` | the file is an orphan | none (unresolved) |

## Edges

- `app/main:crate::main -> app/lib:crate::add crate-path`
- `app/test-plain:crate::a_typed_method_call -> app/lib:crate::Counter::bump typed-method`
- `app/test-plain:crate::a_typed_method_call -> app/lib:crate::Counter::new crate-path`
- `app/test-plain:crate::common::fresh -> app/test-plain:crate::common::start local`
- `app/test-plain:crate::common::start -> app/lib:crate::Counter::new crate-path`
- `app/test-plain:crate::dev_dependency_by_crate_path -> probe/lib:crate::check crate-path`
- `app/test-plain:crate::into_common -> app/test-plain:crate::common::fresh local`
- `app/test-plain:crate::own_lib_by_crate_path -> app/lib:crate::add crate-path`
- `app/test-plain:crate::through_a_use_alias -> app/lib:crate::add crate-path`
- `app/test-plain:crate::through_a_use_alias -> app/test-plain:crate::common::fresh use-alias`
- `app/test-suite:crate::suite_case -> app/lib:crate::add crate-path`

## Counts

- edges: 11
- unresolved: 3
- external: 2
- no_source: 0
- method_calls: 0
- macros: 0
- modules: 6
- orphans: 2

`unresolved` is `common::fresh()` in `suite_case` and the `app::add` calls of the two orphans. `external` is `probe::check` from the lib and from the bin.
