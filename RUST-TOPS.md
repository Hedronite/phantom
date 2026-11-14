# Rust-TOPS

Rust-TOPS is a testing and quality protocol for Rust crates. Each crate
states its class, its test layers and their thresholds in a
`rust-tops.yaml` beside its `Cargo.toml`, and `cargo-tops` checks that file
and runs the enabled layers in a fixed order. Protocol and tooling:
<https://github.com/Hedronite/rust-tops>.

## Versions

- **Protocol:** rust-tops 1.1.0, spec
  `https://github.com/Hedronite/rust-tops/blob/v0.2.0/RUST_TOPS.md`, as each
  crate's `protocol` block states it.
- **Gate:** run with cargo-tops 0.2.0, cargo-llvm-cov 0.9.1, cargo-crap
  0.5.0 and cargo-mutants 27.1.0. cargo-tops 0.2.0 enforces no coverage or
  CRAP threshold, so the grade comes from the measured cargo-llvm-cov,
  cargo-crap and cargo-mutants figures, not from the gate's exit.

## Crates and thresholds

Coverage is line, region and per-file line, in percent. CRAP is the
production and agent threshold. Mutation is the kill rate on the diff, in
percent, and the most unexplained survivors.

| Crate | Class | Line | Region | Per-file | CRAP | Mutation |
|---|---|---|---|---|---|---|
| `phantom-fractal` | binary-cli | 80 | 75 | 90 | 6 / 6 | 85, 0 |
| `phantom-fractal-proto` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-lang` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-lang-rust` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-extract` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-analyze` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-store` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |
| `phantom-fractal-git` | library-core | 95 | 90 | 90 | 6 / 6 | 95, 0 |

Each crate's yaml governs. Coverage, CRAP and mutation are on in every
crate.

## Running the gate

From a crate's directory:

```sh
cargo-tops check   # validate rust-tops.yaml against the protocol schema
cargo-tops gate    # fmt, clippy, deny, nextest, coverage, CRAP, mutation, advisories
```

Call the `cargo-tops` binary directly, not `cargo tops`. Cargo passes
`tops` as the first argument to a subcommand binary, and cargo-tops 0.2.0
does not strip it, so `cargo tops gate` fails with
`unrecognized subcommand 'tops'`.

An exit of 0 is not a pass. The coverage layer writes
`target/coverage/lcov.info` and the CRAP layer writes `target/crap.json`;
neither applies a threshold. The figures are read from the cargo-llvm-cov
summary, `target/crap.json` and the cargo-mutants report, and compared
with the crate's yaml. Mutation runs on the diff against `origin/main`.

## The CRAP rule

CRAP is CC² × (1 − coverage)³ + CC, so it is never below a function's
cyclomatic complexity.

1. Every production function added or changed since `61236dc` has CRAP at
   or below its crate's threshold.
2. No existing function's CRAP rises above its value at `61236dc`.
3. The per-function list at `61236dc` (187 production functions over 6),
   kept with the project's quality measurements, is a burn-down list and
   is not graded. It moves to `target/crap-baseline.json` once cargo-crap
   supports a baseline file.
4. A new or changed exhaustive enum dispatch match (one arm per variant,
   no guards, each arm one call or expression) that cannot reach the
   threshold without a split made only for CRAP takes an `exemptions`
   entry in its crate's yaml, with layer `crap`, a reason and an expiry.

No function is split only to lower its CRAP.

## Exemptions in force

None.

## Known deviations

- **Fuzz and property runs.** A std-only fuzz harness and map property
  run, driven by a seeded splitmix64 generator, stand in for cargo-fuzz
  and proptest. They add no package and need no nightly toolchain.
- **No CI quality job.** Coverage, CRAP and mutation run on the build box,
  not in CI.
- **CLI crate CRAP.** `phantom-fractal` holds CRAP 6, tighter than the
  binary-cli class default of 8.
