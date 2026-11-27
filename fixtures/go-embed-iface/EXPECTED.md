---
title: Expected impact — go-embed-iface fixture
type: fixture
status: ground-truth
created: "2026-11-27"
updated: "2026-11-27"
owner: backend
spec: "[[foundry/phantom/specs/SPEC-w17]]"
cross_refs:
  - "[[foundry/phantom/checklists/CHECKLIST-w17]]"
  - "[[foundry/phantom/rubrics/RUBRIC-w17]]"
  - "[[foundry/phantom/w17]]"
options: "max_depth 3, min_confidence 0.2, include_tests true"
tags:
  - phantom
  - fixture
  - w17
  - go
---

# Expected impact — `fixtures/go-embed-iface`

Ground truth for [[foundry/phantom/specs/SPEC-w17]] G3b. Verified by
`crates/phantom-fractal-analyze/tests/go_fixtures.rs`.

## Cases

## Target Base.Hello

- risk: low
- exact: false
- reasons: external_users_possible, interface_dispatch
- counts: direct 1, total 1, packages 1, tests 0, below_floor 0
- hit: 1 shapes/shapes.go:35 Use 0.95 embedded-method shapes/shapes.go:36

## Target Shape.Area

- risk: low
- exact: false
- reasons: external_users_possible, interface_dispatch
- counts: direct 1, total 1, packages 1, tests 0, below_floor 0
- hit: 1 shapes/shapes.go:35 Use 1 interface-method shapes/shapes.go:36

## Target Rect.Area

- risk: low
- exact: false
- reasons: external_users_possible, interface_dispatch
- counts: direct 0, total 0, packages 0, tests 0, below_floor 0

## Target Rect.UniqueStep

- risk: low
- exact: false
- reasons: external_users_possible, interface_dispatch
- counts: direct 1, total 1, packages 1, tests 0, below_floor 0
- hit: 1 shapes/shapes.go:35 Use 0.7 unique-method shapes/shapes.go:36
