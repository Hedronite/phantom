---
title: Expected details — detail fixture
type: fixture
status: ground-truth
created: "2026-10-28"
updated: "2026-10-28"
owner: fullstack
spec: "[[foundry/phantom/specs/SPEC-w6]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w6]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w6]]"
  - "[[foundry/phantom/w6]]"
tags:
  - phantom
  - fixture
  - w6
---

# Expected details — `fixtures/detail`

Ground truth for [[foundry/phantom/specs/SPEC-w6]] G0a. The test
`crates/phantom-fractal-extract/tests/detail.rs` walks this package,
extracts every `.rs` file, and asserts that the rows below equal every
item and method it finds, in walk order: per file, items in source order,
then methods. Each row's `line`, `end_line`, `signature` and `doc_summary`
must match exactly.

## Line format

These lines are parsed.

- `## File PATH` starts a file. `PATH` is relative to this directory.
- `### KIND NAME` starts a row. `KIND` is an item kind or `method`. A method's `NAME` is `Owner::name`.
- `- line: N` and `- end_line: N` are 1-based.
- `- signature: TEXT` runs to the end of the line.
- `- doc_summary: TEXT` runs to the end of the line, or is `none` for `None`.

## Cases

- **Generics and `where`.** `largest` has a lifetime, type parameters, an `impl Fn` argument and a `where` clause. `Grid` has a const generic and a `where` clause before its field list.
- **Tuple, unit, enum, trait.**
  - `Meters` is a tuple struct. Its tuple fields are its field list, so they are left out.
  - `Stop` is a unit struct and stops before its `;`.
  - `Heading` is an enum, and `Area` is a trait. Neither signature includes its body.
- **Methods.** `Grid::filled` and `Grid::len` are `impl` methods. `Meters::area` is a trait method.
- **Multi-line signature.** `connect` spans five lines before its body. `largest`'s `where` clause spans four.
- **Attributes between doc and item.** `Grid` has two, `Meters`, `Heading`, `noted` and `Meters::area` have one each, and `noted` also has a plain comment after its doc.
- **Block docs.** `Heading` uses a starred multi-line `/** */` block, and `Area` uses a one-line block.
- **Sentences and paragraphs.**
  - `largest` keeps the first sentence of its first paragraph.
  - `connect`'s first sentence spans two `///` lines.
  - `Stop`'s doc has no full stop, so the whole paragraph is kept.
  - `helpers` shows that a `.` followed by a space ends the sentence, even inside `e.g.`.
- **Not docs.**
  - The `//!` lines at the top of `src/lib.rs` never count, so `long` has no summary.
  - `zero` has only `#[doc = ..]`.
  - `quiet` has only a plain comment and a `////` comment.
  - `shapes`, `tiny` and `Grid::len` have nothing above them.
- **Caps.**
  - `configure_everything`'s signature is 267 chars, so it is cut at 256 and `…` is appended.
  - `fits_the_cap`'s signature is exactly 256 chars, so it is not cut.
  - `summarize`'s first sentence is 176 chars. Its 160th char is the `é` of `café`, so the cut keeps that `é` and appends `…`.

## File src/lib.rs

### module long

- line: 4
- end_line: 4
- signature: pub mod long
- doc_summary: none

### module shapes

- line: 5
- end_line: 5
- signature: pub mod shapes
- doc_summary: none

### function largest

- line: 10
- end_line: 23
- signature: pub fn largest<'a, T, K>(items: &'a [T], key: impl Fn(&T) -> K) -> Option<&'a T> where T: Clone, K: Ord,
- doc_summary: Finds the largest item by key.

### function connect

- line: 27
- end_line: 36
- signature: pub(crate) fn connect( host: &str, port: u16, retries: u8, ) -> Result<u16, String>
- doc_summary: Opens a session on the named host and port.

### function zero

- line: 39
- end_line: 41
- signature: pub const fn zero() -> u32
- doc_summary: none

### function quiet

- line: 45
- end_line: 45
- signature: fn quiet()
- doc_summary: none

### function noted

- line: 50
- end_line: 50
- signature: fn noted()
- doc_summary: Kept across the plain comment below.

### module helpers

- line: 54
- end_line: 58
- signature: mod helpers
- doc_summary: Helpers for the tests, e.g.

### function tiny

- line: 55
- end_line: 57
- signature: pub fn tiny() -> u8
- doc_summary: none


## File src/long.rs

### function configure_everything

- line: 4
- end_line: 13
- signature: pub fn configure_everything( first_argument_with_a_long_name: u64, second_argument_with_a_long_name: u64, third_argument_with_a_long_name: u64, fourth_argument_with_a_long_name: u64, fifth_argument_with_a_long_name: u64, sixth_argument_with_a_long_name: u6…
- doc_summary: Takes far too many arguments.

### function fits_the_cap

- line: 16
- end_line: 32
- signature: pub fn fits_the_cap( alpha_value: u64, beta_value: u64, gamma_value: u64, delta_value: u64, epsilon_value: u64, zeta_value: u64, eta_value: u64, theta_value: u64, iota_value: u64, kappa_value: u64, lambda_value: u64, omicron_value: u64, mu_v: u64, ) -> u64
- doc_summary: Exactly at the signature cap, so nothing is cut.

### function summarize

- line: 35
- end_line: 37
- signature: pub fn summarize() -> &'static str
- doc_summary: Résumé of a run, written for the reader who skims the page: every field is kept as measured, nothing is rounded, and the totals come last, as they did in a café…


## File src/shapes.rs

### struct Meters

- line: 5
- end_line: 5
- signature: pub struct Meters
- doc_summary: Meters along a track.

### struct Stop

- line: 8
- end_line: 8
- signature: pub struct Stop
- doc_summary: Marks the end of a run

### enum Heading

- line: 16
- end_line: 21
- signature: pub enum Heading
- doc_summary: Compass headings, clockwise.

### struct Grid

- line: 26
- end_line: 31
- signature: pub struct Grid<T: Copy, const N: usize> where T: Default,
- doc_summary: A square grid of cells.

### trait Area

- line: 34
- end_line: 42
- signature: pub trait Area: Sized
- doc_summary: Things with an area.

### method Grid::filled

- line: 46
- end_line: 48
- signature: pub fn filled(value: T) -> Self
- doc_summary: Builds a grid with every cell set to one value.

### method Grid::len

- line: 50
- end_line: 52
- signature: fn len(&self) -> usize
- doc_summary: none

### method Meters::area

- line: 58
- end_line: 60
- signature: fn area(&self) -> f64
- doc_summary: Treats the length as a side.
