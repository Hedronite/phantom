---
title: Expected edges — resolve-unique fixture
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

# Expected edges — `fixtures/resolve-unique`

Ground truth for [[foundry/phantom/specs/SPEC-w10]] G3a: the `unique-method` tier. The test `crates/phantom-fractal-extract/tests/resolve_unique.rs` indexes a copy of this package. It asserts that the stored edge set equals the **Edges** list exactly, that every **Counts** line matches, and that the per-reason counts equal the reasons in **Edges**.

A symbol is written `<target>:<module path>::<name>`, with the impl owner before a method's name. The only target is `lib` (`src/lib.rs`). Each line is `caller -> callee reason`.

## Cases

| Function | Call | Why | Edge |
|---|---|---|---|
| `one_unique_name` | `make().settle()` | no hint (the receiver is a call), and `settle` is the one workspace method of that name | `unique-method` to `Ledger::settle` |
| `a_field_receiver` | `s.ledger.settle()` | no hint (the receiver is a field) | `unique-method` to `Ledger::settle` |
| `a_cleared_hint` | `|x| x.settle()` | a closure parameter clears the hint | `unique-method` to `Ledger::settle` |
| `a_stoplisted_name` | `make().len()` | `len` is on `STD_METHOD_NAMES`, though `Ledger::len` is the one workspace `len` | none |
| `a_name_defined_twice` | `make().label()` | `Ledger::label` and `Book::label` | none |
| `a_hint_that_places_nothing` | `v.settle()` with `v: Vec<u8>` | the receiver has a hint, `type:Vec`, which places nothing; only an unhinted call takes the name rule | none |
| `a_typed_call_stays_typed` | `l.settle()` with `l: Ledger` | the hint places it | `typed-method` to `Ledger::settle` |
| `Book::shelve` | `self.settle()` | a `self` receiver takes only the `self-method` tier, and `Book` has no `settle` | none (unresolved) |

## Edges

- `lib:crate::a_cleared_hint -> lib:crate::Ledger::settle unique-method`
- `lib:crate::a_field_receiver -> lib:crate::Ledger::settle unique-method`
- `lib:crate::a_name_defined_twice -> lib:crate::make local`
- `lib:crate::a_stoplisted_name -> lib:crate::make local`
- `lib:crate::a_typed_call_stays_typed -> lib:crate::Ledger::settle typed-method`
- `lib:crate::one_unique_name -> lib:crate::Ledger::settle unique-method`
- `lib:crate::one_unique_name -> lib:crate::make local`

## Counts

- edges: 7
- unresolved: 1
- external: 0
- no_source: 0
- method_calls: 3
- macros: 0
- modules: 1
- orphans: 0

`unresolved` is `self.settle()` in `Book::shelve`. `method_calls` is `make().len()`, `make().label()` and `v.settle()`.
