---
title: Expected links — graph fixture
type: fixture
status: ground-truth
created: "2026-10-28"
updated: "2026-10-28"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w7]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w7]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w7]]"
tags:
  - phantom
  - fixture
  - w7
---

# Expected links — `fixtures/graph`

Ground truth for [[foundry/phantom/specs/SPEC-w7]] G0a. The test
`crates/phantom-fractal-extract/tests/links.rs` indexes a copy of this
workspace and asserts that its `import` and `impl` links equal the rows of
**Import edges** and **Impl edges** together, exactly. Call edges are not
listed here.

A node is `module <target>:<module path>` or `symbol <target>:<module
path>::<name>`, where the target is `<package>/<root file stem>`. Each row is
`kind source -> target reason confidence`, with the confidence to two places.

## Import edges

- `import module app/lib:crate::shapes -> symbol app/lib:crate::traits::Named path-exact 1.00`
- `import module app/lib:crate::shapes -> symbol kit/lib:crate::Shape crate-path 0.95`
- `import module app/lib:crate::uses -> module app/lib:crate::shapes path-exact 1.00`
- `import module app/lib:crate::uses -> symbol app/lib:crate::shapes::Circle path-exact 1.00`
- `import module app/lib:crate::uses -> symbol app/lib:crate::shapes::Circle use-alias 0.95`
- `import module app/lib:crate::uses -> symbol app/lib:crate::shapes::Square path-exact 1.00`
- `import module app/lib:crate::uses -> symbol app/lib:crate::shapes::Tile path-exact 1.00`
- `import module app/lib:crate::uses -> symbol app/lib:crate::shapes::Wrapper path-exact 1.00`
- `import module app/lib:crate::uses -> symbol app/lib:crate::uses::inner::deep local 0.95`
- `import module app/lib:crate::uses -> symbol kit/lib:crate::geo::Point crate-path 0.95`
- `import module app/lib:crate::uses -> symbol kit/lib:crate::scale crate-path 0.95`
- `import module app/lib:crate::uses::inner -> symbol app/lib:crate::shapes::Circle path-exact 1.00`

## Impl edges

- `impl symbol app/lib:crate::shapes::Circle -> symbol app/lib:crate::traits::Named impl-trait 0.95`
- `impl symbol app/lib:crate::shapes::Circle -> symbol kit/lib:crate::Shape impl-trait 0.95`
- `impl symbol app/lib:crate::shapes::Square -> symbol kit/lib:crate::geo::Describe impl-trait 0.95`
- `impl symbol app/lib:crate::shapes::Tile -> symbol app/lib:crate::traits::Named impl-trait 1.00`
- `impl symbol app/lib:crate::shapes::Tile -> symbol kit/lib:crate::Shape impl-trait 0.95`
- `impl symbol app/lib:crate::shapes::Wrapper -> symbol app/lib:crate::traits::Named impl-trait 0.95`

## Cases

- **Plain.** `use crate::shapes::Circle;` in `uses`: `path-exact`.
- **Aliased.** `use crate::shapes::Square as Block;` links to `Square`; the alias changes only the local name. `use kit::scale as grow;` does the same across crates.
- **Grouped.** `use crate::shapes::{Tile, Wrapper};` gives one link per leaf.
- **Module.** `use crate::shapes;` links `uses` to the module `crate::shapes`, not to its `mod` symbol.
- **Cross-crate.** `use kit::geo::Point;` and `use kit::Shape;` enter `kit` by crate name through `app`'s path dependency: `crate-path`.
- **Use-alias tier.** `use shapes::Circle as Round;` starts at the name the `use crate::shapes;` leaf binds, so it is `use-alias`. It is a second leaf naming `Circle`, so it is a second row.
- **Local tier.** `use inner::deep;` starts at the child module `inner`: `local`.
- **Inline module.** `use super::Circle;` inside `mod inner` links from `crate::uses::inner`, the module it sits in. `super` makes it `path-exact`.
- **`impl Trait for Struct`.** `Circle` implements `Named` from another module through `use crate::traits::Named` (`0.95`, the `use-alias` tier), and `Shape` from another crate through `use kit::Shape` (`0.95`, `crate-path`). `Square` implements `kit::geo::Describe` by its full path (`crate-path`).
- **`impl Trait for Enum`.** `Tile` implements `crate::traits::Named` (`1.00`, `path-exact`) and `kit::Shape` (`crate-path`).
- **Two impls of one trait.** `Named` has links from `Circle`, `Tile` and `Wrapper`.
- **Deduplicated.** `Circle`'s `Shape` impl has two methods and gives one link. `Wrapper<u8>` and `Wrapper<u16>` each implement `Named` and give one link.

## No edge

| Written | In | Why |
|---|---|---|
| `use crate::traits::*;` | `dup` | A glob gives no link. |
| `use std::collections::HashMap;` | `uses` | External. |
| `use super::missing::Gone;` | `uses` | Unresolved: the crate root has no `missing`. |
| `use crate::dup::Dup;` | `uses` | Ambiguous: `Dup` is a struct and a function, two symbols. |
| `impl Missing for Square` | `shapes` | Unresolved trait: nothing in scope binds `Missing`. |
| `impl std::fmt::Display for Square` | `shapes` | External trait. |
| `impl Clone for Tile` | `shapes` | A prelude trait, outside the workspace: external. |
| `impl Named for Dup` | `dup` | `Named` is only in scope through the glob, which no tier follows. |
| `impl Circle` | `shapes` | Inherent: no trait. |
