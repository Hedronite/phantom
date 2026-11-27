---
title: Expected impact — go-testpkg fixture
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

# Expected impact — `fixtures/go-testpkg`

Ground truth for [[foundry/phantom/specs/SPEC-w17]] G3b. Verified by
`crates/phantom-fractal-analyze/tests/go_fixtures.rs`.

## Cases

## Target example.com/gotestpkg/internal/calc.OnlyXTest

- risk: low
- exact: true
- reasons: none
- counts: direct 0, total 0, packages 0, tests 1, below_floor 0
- test: 1 internal/calc/calc_external_test.go:9 TestExternal 1 package-import internal/calc/calc_external_test.go:10

## Target example.com/gotestpkg/internal/calc.Add

- risk: low
- exact: true
- reasons: none
- counts: direct 1, total 1, packages 1, tests 1, below_floor 0
- hit: 1 internal/calc/calc_internal_test.go:5 helper 1 local internal/calc/calc_internal_test.go:6
- test: 2 internal/calc/calc_internal_test.go:9 TestInternal 1 local internal/calc/calc_internal_test.go:10

## Target example.com/gotestpkg/internal/calc.Platform

- risk: low
- exact: false
- reasons: build_excluded=2
- counts: direct 0, total 0, packages 0, tests 0, below_floor 0
