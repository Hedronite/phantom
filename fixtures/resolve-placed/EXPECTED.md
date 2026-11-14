---
title: Expected edges — resolve-placed fixture
type: fixture
status: ground-truth
created: "2026-11-14"
updated: "2026-11-14"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w12]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w12]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w12]]"
  - "[[foundry/phantom/w12]]"
tags:
  - phantom
  - fixture
  - w12
---

# Expected edges — `fixtures/resolve-placed`

Ground truth for [[foundry/phantom/specs/SPEC-w12]] G2a: receivers placed by the D6 cures. The test `crates/phantom-fractal-extract/tests/resolve_placed.rs` indexes a copy of this package. It asserts three things exactly: the stored edge set equals the **Edges** list, every **Counts** line matches, and the per-reason counts equal the reasons in **Edges**. It also asserts the stored method calls listed under **Method calls**.

There is one lib target (`src/lib.rs`), with the modules `crate` and `crate::other` (`src/other.rs`). `Ledger` and `Book` both have a method `label`, so only a hint places `label`. A symbol is written `<target>:<module path>::<name>`, with the impl owner before a method's name. Each line is `caller -> callee reason`.

## Cases

| Caller | Call | Hint | Why | Edge |
|---|---|---|---|---|
| `ctor_other` | `let b = Ledger::shelve(); b.label()` | `ctor:Ledger::shelve` | (d): `shelve` returns `Book`, another workspace type | `typed-method` to `Book::label` |
| `returned` | `let b = make_book(); b.label()` | `ret:make_book` | (b): `make_book` returns `Book` | `typed-method` to `Book::label` |
| `returned_tried` | `let b = find_book()?; b.label()` | `ret?:find_book` | (b): `find_book` returns `Option<Book>` | `typed-method` to `Book::label` |
| `call_receivers` | `make_book().label()` | `ret:make_book` | (c), by rule (b) | `typed-method` to `Book::label` |
| `call_receivers` | `Ledger::shelve().label()` | `ctor:Ledger::shelve` | (c), by rule (d) | `typed-method` to `Book::label` |
| `Shelf::same_file_field` | `self.ledger.label()` | `type:Ledger` | (a): `Shelf` is in this file, with `ledger: Box<Ledger>` | `typed-method` to `Ledger::label` |
| `Wrap::generic_field` | `self.inner.label()` | none | `inner: T` is a generic parameter of `Wrap` | none |
| `Far::other_file_field` | `self.book.label()` | none | `Far` is defined in `src/other.rs` | none |
| `method_chain` | `b.again().label()` | none | a method chain is one level too deep | none |
| `method_chain` | `b.again()` | `ret:make_book` | `again` is the one workspace method of that name, so the name rule places it first | `unique-method` to `Book::again` |
| `keeps_unique` | `make_book().settle()` | `ret:make_book` | `settle` is unique, so the call keeps its `unique-method` edge; the hint is never tried | `unique-method` to `Book::settle` |
| `shadowed_fn` | `let b = make_book(); b.label()` | none | `let make_book = \|\| …` binds the name in the function | none |

## Edges

- `lib:crate::Shelf::same_file_field -> lib:crate::Ledger::label typed-method`
- `lib:crate::call_receivers -> lib:crate::Book::label typed-method`
- `lib:crate::call_receivers -> lib:crate::Book::label typed-method`
- `lib:crate::call_receivers -> lib:crate::Ledger::shelve assoc`
- `lib:crate::call_receivers -> lib:crate::make_book local`
- `lib:crate::ctor_other -> lib:crate::Book::label typed-method`
- `lib:crate::ctor_other -> lib:crate::Ledger::shelve assoc`
- `lib:crate::keeps_unique -> lib:crate::Book::settle unique-method`
- `lib:crate::keeps_unique -> lib:crate::make_book local`
- `lib:crate::method_chain -> lib:crate::Book::again unique-method`
- `lib:crate::method_chain -> lib:crate::make_book local`
- `lib:crate::returned -> lib:crate::Book::label typed-method`
- `lib:crate::returned -> lib:crate::make_book local`
- `lib:crate::returned_tried -> lib:crate::Book::label typed-method`
- `lib:crate::returned_tried -> lib:crate::find_book local`
- `lib:crate::shadowed_fn -> lib:crate::Ledger::open assoc`

## Counts

- edges: 16
- unresolved: 1
- external: 2
- no_source: 0
- method_calls: 4
- macros: 0
- modules: 2
- orphans: 0

`unresolved` is `make_book()` in `shadowed_fn`, which the closure shadows. `external` is the two `Some(..)`. `method_calls` is the four `label` calls with no edge: in `generic_field`, `other_file_field`, `method_chain` and `shadowed_fn`.

## Method calls

Every stored method call as `caller text receiver hint`, with `-` for none:

- `call_receivers label Ledger::shelve() ctor:Ledger::shelve`
- `call_receivers label make_book() ret:make_book`
- `ctor_other label b ctor:Ledger::shelve`
- `generic_field label self.inner -`
- `keeps_unique settle make_book() ret:make_book`
- `method_chain again b ret:make_book`
- `method_chain label b.again() -`
- `other_file_field label self.book -`
- `returned label b ret:make_book`
- `returned_tried label b ret?:find_book`
- `same_file_field label self.ledger type:Ledger`
- `shadowed_fn label b -`
