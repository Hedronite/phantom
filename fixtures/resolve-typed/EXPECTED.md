---
title: Expected edges — resolve-typed fixture
type: fixture
status: ground-truth
created: "2026-10-30"
updated: "2026-10-30"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w10]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w10]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w10]]"
tags:
  - phantom
  - fixture
  - w10
---

# Expected edges — `fixtures/resolve-typed`

Ground truth for [[foundry/phantom/specs/SPEC-w10]] G2a: method calls on a typed local receiver. The test `crates/phantom-fractal-extract/tests/resolve_typed.rs` indexes a copy of this workspace. It asserts that the edge set equals the **Edges** list exactly, that every **Counts** line matches, and that every `typed-method` edge has confidence 0.8.

A symbol is written `<package>/<target>:<module path>::<name>`, with the impl owner before a method's name. Each line is `caller -> callee reason`. `typed/src/lib.rs` holds the cases that give an edge, and `typed/src/clears.rs` the bindings that clear a hint.

## Cases

| Function | Binding of the receiver | Hint | Edge |
|---|---|---|---|
| `by_param` | parameter `c: Counter` | `type:Counter` | `Counter::read` |
| `by_ref` | parameter `c: &Counter` | `type:Counter` | `Counter::read` |
| `by_mut` | parameter `c: &mut Counter` | `type:Counter` | `Counter::bump` |
| `by_box`, `by_rc`, `by_arc` | parameter `Box<Counter>`, `Rc<Counter>`, `Arc<Counter>` | `type:Counter` | `Counter::read` |
| `by_scoped` | parameter `c: crate::Counter` | `type:crate::Counter` | `Counter::read` |
| `by_generic` | parameter `w: Wrap<u8>`, generic arguments dropped | `type:Wrap` | `Wrap::get` |
| `let_typed` | `let c: Counter = make()` | `type:Counter` | `Counter::read` |
| `let_wrapped` | `let g: Arc<Gauge>`; `let c: &mut Box<Gauge>` unwraps one level only | `type:Gauge`; none | `Gauge::read` for `g` only |
| `let_literal` | `let c = Counter { .. }` | `type:Counter` | `Counter::read` |
| `ctor_self` | `let c = Counter::new()`, which returns `Self` | `ctor:Counter::new` | `Counter::read` |
| `ctor_named` | `let c = Counter::fresh()`, which returns `Counter` | `ctor:Counter::fresh` | `Counter::read` |
| `ctor_result` | `let c = Counter::open()?`, which returns `Result<Self, Error>` | `ctor?:Counter::open` | `Counter::read` |
| `ctor_option` | `let c = Counter::find()?`, which returns `Option<Counter>` | `ctor?:Counter::find` | `Counter::read` |
| `ctor_not_self` | `let n = Counter::count()`, which returns `u32` | `ctor:Counter::count` | none |
| `ctor_untried` | `let c = Counter::open()` without `?`, which returns a `Result` | `ctor:Counter::open` | none |
| `Counter::twin` | `let other = Self::new()` in `impl Counter` | `ctor:Counter::new` | `Counter::read` |
| `Counter::merge` | parameter `other: Self` | `type:Counter` | `Counter::read` |
| `Counter::literal` | `let made = Self { .. }` | `type:Counter` | `Counter::read` |
| `far` | parameter `p: Point`, `use geo::Point` from another workspace crate | `type:Point` | `geo` `Point::norm` |
| `far_ctor` | `let p = geo::Point::new()` | `ctor:geo::Point::new` | `geo` `Point::norm` |
| `shadowed` | `let c = Gauge::new()` after parameter `c: Counter` | `ctor:Gauge::new` | `Gauge::read` |
| `shadow_ends` | `let c = Gauge::new()` in an inner block, then the parameter `c: Counter` after it | `ctor:Gauge::new`, then `type:Counter` | `Gauge::read`, then `Counter::read` |
| `in_closure` | parameter `c: Counter`, the call in a closure body | `type:Counter` | `Counter::read` |
| `two_traits` | parameter `c: Counter`; `go` comes from both `impl Left` and `impl Right` | `type:Counter` | none |
| `trait_impl_method` | parameter `r: Gauge`; `peek` comes only from `impl Reader for Gauge` | `type:Gauge` | `Gauge::peek` |
| `clears::untyped_let` | `let c = make()` | none | none |
| `clears::inferred_let` | `let c: _ = make()` | none | none |
| `clears::in_match` | `Some(c)` in a `match` arm | none | none |
| `clears::in_if_let` | `if let Some(c)` | none | none |
| `clears::in_while_let` | `while let Some(c)` | none | none |
| `clears::in_for` | `for c in all` | none | none |
| `clears::closure_parameter` | closure parameter `|c: Counter|` | none | none |
| `clears::untyped_closure` | closure parameter `|c|` | none | none |
| `clears::tuple_pattern` | `let (c, _) = pair` | none | none |
| `clears::struct_pattern` | `let Holder { c } = h` | none | none |
| `clears::impl_trait` | parameter `c: impl Reader` | none | none |
| `clears::dyn_trait` | parameter `c: &dyn Reader` | none | none |

Every function in `clears.rs` also has a parameter `c: Counter`, apart from `impl_trait` and `dyn_trait`. So each clearing case is a nearer binding that overrides a hint that would have placed an edge.

## Edges

- `typed/lib:crate::Counter::literal -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::Counter::merge -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::Counter::twin -> typed/lib:crate::Counter::new self-assoc`
- `typed/lib:crate::Counter::twin -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_arc -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_box -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_generic -> typed/lib:crate::Wrap::get typed-method`
- `typed/lib:crate::by_mut -> typed/lib:crate::Counter::bump typed-method`
- `typed/lib:crate::by_param -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_rc -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_ref -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::by_scoped -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::clears::inferred_let -> typed/lib:crate::make use-alias`
- `typed/lib:crate::clears::untyped_let -> typed/lib:crate::make use-alias`
- `typed/lib:crate::ctor_named -> typed/lib:crate::Counter::fresh assoc`
- `typed/lib:crate::ctor_named -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::ctor_not_self -> typed/lib:crate::Counter::count assoc`
- `typed/lib:crate::ctor_option -> typed/lib:crate::Counter::find assoc`
- `typed/lib:crate::ctor_option -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::ctor_result -> typed/lib:crate::Counter::open assoc`
- `typed/lib:crate::ctor_result -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::ctor_self -> typed/lib:crate::Counter::new assoc`
- `typed/lib:crate::ctor_self -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::ctor_untried -> typed/lib:crate::Counter::open assoc`
- `typed/lib:crate::far -> geo/lib:crate::Point::norm typed-method`
- `typed/lib:crate::far_ctor -> geo/lib:crate::Point::new crate-path`
- `typed/lib:crate::far_ctor -> geo/lib:crate::Point::norm typed-method`
- `typed/lib:crate::in_closure -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::let_literal -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::let_typed -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::let_typed -> typed/lib:crate::make local`
- `typed/lib:crate::let_wrapped -> typed/lib:crate::Gauge::read typed-method`
- `typed/lib:crate::make -> typed/lib:crate::Counter::new assoc`
- `typed/lib:crate::shadow_ends -> typed/lib:crate::Counter::read typed-method`
- `typed/lib:crate::shadow_ends -> typed/lib:crate::Gauge::new assoc`
- `typed/lib:crate::shadow_ends -> typed/lib:crate::Gauge::read typed-method`
- `typed/lib:crate::shadowed -> typed/lib:crate::Gauge::new assoc`
- `typed/lib:crate::shadowed -> typed/lib:crate::Gauge::read typed-method`
- `typed/lib:crate::trait_impl_method -> typed/lib:crate::Gauge::peek typed-method`

## Counts

- edges: 39
- unresolved: 1
- external: 5
- no_source: 0
- method_calls: 17
- macros: 0
- modules: 3
- orphans: 0

`unresolved` is the call `f()` in `in_closure`, whose name a `let` binds. `external` is the two `todo()`, the two `Ok(..)` and the `Some(..)`. `method_calls` is the 17 calls on a receiver other than `self` that got no edge: 4 in `lib.rs` (`let_wrapped`'s `c`, `ctor_not_self`, `ctor_untried`, `two_traits`) and 13 in `clears.rs`, one per function plus `o.pop()`.
