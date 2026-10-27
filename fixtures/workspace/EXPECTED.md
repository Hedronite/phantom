---
title: Expected edges — workspace fixture
type: fixture
status: ground-truth
created: "2026-10-27"
updated: "2026-10-27"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w4]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w4]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w4]]"
tags:
  - phantom
  - fixture
  - w4
---

# Expected edges — `fixtures/workspace`

Ground truth for [[foundry/phantom/specs/SPEC-w4]] G2c. The test
`crates/phantom-fractal-extract/tests/workspace.rs` indexes a copy of this
workspace and asserts three things: the stored edge set equals the **Edges**
list exactly, every **Counts** line matches, and the per-reason edge counts
equal the reasons in **Edges**.

A symbol is written `<package>/<target>:<module path>::<name>`, with the impl
owner before a method's name. A target is its root file's stem: `lib` for
`src/lib.rs`, and `main` for `src/main.rs`. Each line is `caller -> callee reason`.

## Edges

- `core-lib/lib:crate::f -> core-lib/lib:crate::util::helper local`
- `core-lib/lib:crate::Thing::new -> core-lib/lib:crate::Thing::make self-assoc`
- `app/lib:crate::run -> core-lib/lib:crate::f crate-path`
- `app/lib:crate::run -> core-lib/lib:crate::util::helper crate-path`
- `app/lib:crate::run -> core-lib/lib:crate::util::helper crate-path`
- `app/lib:crate::run -> core-lib/lib:crate::util::helper crate-path`
- `app/lib:crate::run -> core-lib/lib:crate::Thing::new crate-path`
- `app/lib:crate::run -> core-lib/lib:crate::Thing::new crate-path`
- `app/lib:crate::Thing::twice -> app/lib:crate::Thing::show self-method`
- `app/main:crate::main -> app/lib:crate::run crate-path`
- `app/main:crate::main -> core-lib/lib:crate::f crate-path`
- `uses-ws/lib:crate::go -> core-lib/lib:crate::f crate-path`
- `ws-inline/lib:crate::go -> core-lib/lib:crate::util::helper crate-path`
- `shadow/lib:crate::go -> shadow/lib:crate::core_lib::f local`

## Cases

- **`app`'s lib** has the path dependency `core-lib = { path = "../core-lib", … }`. Its `run` reaches `core-lib` by crate path in four ways:
  - directly, as `core_lib::f()`, `core_lib::util::helper()`, and `core_lib::Thing::new()`;
  - through the re-export `core_lib::again()`;
  - through `use core_lib::util::helper` (`helper()`);
  - through `use core_lib::Thing as CoreThing` (`CoreThing::new()`).

  The three `util::helper` lines and the two `Thing::new` lines are separate call sites.
- **`app`'s bin** calls its own package's lib, `app::run()`, and `app`'s dependency, `core_lib::f()`.
- **`uses-ws`** reaches `core-lib` through `core-lib.workspace = true`. **`ws-inline`** reaches it through `core-lib = { workspace = true, … }`. Both forms are backed by the root's `[workspace.dependencies]` path entry.
- **`shadow`** depends on `core-lib`, but its local `mod core_lib` wins, so the edge is `local`.
- **`core-lib`'s own edges:** `f` calls its child module `util`, which is `local`. `Thing::new` calls `Self::make()`, which is `self-assoc`.
- **`app`'s trait impl:** `impl Show for core_lib::Thing` resolves its self type across the crate. `self.show()` reaches the one `show`, which is in `app`'s own target.

## No edge

| Call | In | Why |
|---|---|---|
| `core_lib::f()` | `core-lib` `own` | A lib calling its own crate by name: external. |
| `core_lib::missing()` | `app` `run` | The path enters `core-lib` but names nothing there: unresolved. |
| `app::run()` | `app` lib `run` | A lib calling its own crate by name: external. |
| `core_lib::f()` | `no-dep` `go` | No dependency on `core-lib`: external. |
| `corelib::f()` | `renamed` `go` | A `package = "core-lib"` rename gives no dependency: external. |
| `core_lib::f()` | `renamed` `go` | No dependency under that name: external. |
| `core_lib::f()` | `dev-only` `go` | A `[dev-dependencies]` entry gives no dependency: external. |

## Counts

- edges: 14
- unresolved: 1
- external: 6
- no_source: 0
- method_calls: 0
- macros: 0
- modules: 11
- orphans: 0
