---
title: Expected impact — go-basic fixture
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

# Expected impact — `fixtures/go-basic`

Ground truth for [[foundry/phantom/specs/SPEC-w17]] G3b. Verified by
`crates/phantom-fractal-analyze/tests/go_fixtures.rs`.

## Cases

## Target example.com/gobasic/internal/core.Leaf

- risk: low
- exact: true
- reasons: none
- counts: direct 1, total 4, packages 1, tests 0, below_floor 0
- hit: 1 internal/core/core.go:7 Mid 1 local internal/core/core.go:8
- hit: 2 internal/core/core.go:11 Helper 1 local internal/core/core.go:12
- hit: 2 lib/lib.go:21 Compute 1 package-import lib/lib.go:23
- hit: 3 cmd/app/main.go:5 Run 1 package-import cmd/app/main.go:6

## Target example.com/gobasic/lib.hook

- risk: unknown
- exact: false
- reasons: unresolved_refs=1, value_reference=1
- counts: direct 0, total 0, packages 0, tests 0, below_floor 0

## Target Svc.Boot

- risk: low
- exact: false
- reasons: external_users_possible, interface_dispatch
- counts: direct 1, total 3, packages 1, tests 0, below_floor 0
- hit: 1 lib/lib.go:21 Compute 1 typed-method lib/lib.go:23
- hit: 2 cmd/app/main.go:5 Run 1 package-import cmd/app/main.go:6
- hit: 3 cmd/app/main.go:9 main 1 local cmd/app/main.go:10

## Target example.com/gobasic/lib.Compute

- risk: low
- exact: false
- reasons: external_users_possible
- counts: direct 1, total 2, packages 1, tests 0, below_floor 0
- hit: 1 cmd/app/main.go:5 Run 1 package-import cmd/app/main.go:6
- hit: 2 cmd/app/main.go:9 main 1 local cmd/app/main.go:10

## Target example.com/gobasic/cmd/app.Run

- risk: low
- exact: true
- reasons: none
- counts: direct 1, total 1, packages 1, tests 0, below_floor 0
- hit: 1 cmd/app/main.go:9 main 1 local cmd/app/main.go:10
