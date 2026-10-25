# Phantom

Phantom is a Rust code-graph engine.

Pre-release. Version 0.0.0. Nothing here is published from this tree.

## Draft slice

The first slice lands in steps G0 to G3. G0, the scaffold, is built:

- a Cargo workspace on Rust 1.95.0, edition 2024
- the `phantom-fractal` package, which builds two binaries from one `main.rs`: `phantom-fractal` and `fract`
- stub crates for the steps below, with no code yet

Not built yet. Later steps in this slice add:

- G1: a serde result envelope
- G2: a Rust source extractor for items, `mod`, and `use`
- G3: a SQLite round-trip of one extracted file

Both binaries print `phantom-fractal 0.0.0` for `--version` and `--help`. They have no subcommands. You cannot index or query a code graph with this slice.

```
cargo run -p phantom-fractal --bin fract -- --version
```

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. SPDX: `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
