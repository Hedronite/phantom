---
title: Expected edges — resolve-self fixture
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

# Expected edges — `fixtures/resolve-self`

Ground truth for [[foundry/phantom/specs/SPEC-w4]] G1d. The test
`crates/phantom-fractal-extract/tests/resolve_self.rs` indexes a copy of
this package and asserts three things: the stored edge set equals the
**Edges** list exactly, every **Counts** line matches, and the per-reason
edge counts equal the reasons in **Edges**.

A symbol is written `<target>:<module path>::<name>`, with the impl owner
before a method's name. The only target is `lib` (`src/lib.rs`). Each line is
`caller -> callee reason`.

## Edges

- `lib:crate::Counter::new -> lib:crate::Counter::helper self-assoc`
- `lib:crate::Counter::bump -> lib:crate::Counter::check self-method`
- `lib:crate::Counter::bump -> lib:crate::Counter::describe self-method`
- `lib:crate::Counter::bump -> lib:crate::Counter::new self-assoc`
- `lib:crate::Counter::describe -> lib:crate::Counter::check self-method`
- `lib:crate::Counter::unclear -> lib:crate::Counter::check self-method`
- `lib:crate::Wrap::peek -> lib:crate::Wrap::get self-method`
- `lib:crate::entry -> lib:crate::Counter::new assoc`
- `lib:crate::entry -> lib:crate::Counter::new assoc`
- `lib:crate::shapes::Shape::describe -> lib:crate::shapes::Shape::area self-method`
- `lib:crate::shapes::Plot::describe -> lib:crate::shapes::Plot::area self-method`
- `lib:crate::shapes::Plot::describe -> lib:crate::shapes::Plot::area self-assoc`
- `lib:crate::shapes::Counter::from_shapes -> lib:crate::Counter::check self-method`

## Cases

The edges above cover these cases:

- **Self-method:**
  - `self.check()` in `Counter::bump` reaches an inherent method.
  - `self.describe()` in `Counter::bump` reaches the method of `impl Describe for Counter`.
  - `self.check()` in that trait-impl method reaches the inherent `check`.
  - The `self.check()` in `Counter::unclear` sits inside a closure, so the closure's method is the caller.
- **Self-assoc:** `Self::helper()` in `Counter::new`, and `Self::new()` in `Counter::bump`.
- **Same name on two types:** `Shape` and `Plot` each have `area` and `describe`. Each call goes to its own type, for both `self.area()` and `Self::area(self)`.
- **Impl reached by path:** `impl super::Counter` in `src/shapes.rs` resolves to `crate::Counter`, so its `self.check()` reaches `Counter::check`.
- **Generic impl:** `impl<T> Wrap<T>` resolves to `Wrap`.
- **W3 assoc:** the two `Counter::new()` calls in `entry` keep their W3 `assoc` edge.

## No edge

| Call | In | Why |
|---|---|---|
| `self.inner.tick()` | `Counter::bump` | `self.field.m()`: a method call on a value. |
| `other.tick()` | `Counter::bump` | `x.m()`: a method call on a value. |
| `self.describe()` | trait default `Describe::provided` | A trait default body has no symbol: unresolved. |
| `self.both()` | `Counter::unclear` | An inherent `both` and `impl Describe`'s `both`: two candidates. |
| `self.provided()` | `Counter::unclear` | Only the trait's provided default has it: no candidate. |
| `self.check()` | `<Vec<u8> as Display>::fmt` | `Vec<u8>` does not resolve to a workspace type. |
| `Ok(())` | `<Vec<u8> as Display>::fmt` | External: `Ok` is not found. |
| `Self::Circle(1.0)` | `Shape::unit` | A variant, not a method: unresolved. |
| `self.m()` | `twins::Gate::open` | Two `#[cfg]` impls of `m`: two candidates. |
| `Self::m(self)` | `twins::Gate::open` | The same two candidates. |
| `Counter::new().bump()` | `entry` | A method call on a value. |
| `counter.check()` | `entry` | A method call on a value. |

## Counts

- edges: 13
- unresolved: 7
- external: 1
- no_source: 0
- method_calls: 4
- macros: 0
- modules: 3
- orphans: 0
