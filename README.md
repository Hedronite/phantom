# Phantom

Phantom is a Rust code-graph engine.

Pre-release. Version 0.0.0. Nothing here is published from this tree.

## What works today

The `phantom-fractal` package builds two binaries from one `main.rs`: `phantom-fractal` and `fract`. Scripts should call `phantom-fractal`, since another program named `fract` may come first on `PATH`.

They have two commands:

- `index PATH` walks the Rust files under PATH, extracts their items and `use` paths, and stores them in a SQLite database in the Phantom data directory. PATH itself is only read. A second run skips files whose hash has not changed.
- `find NAME` returns the indexed symbols named exactly NAME as `hits`, each with its kind, visibility, path, and line, ordered by path and then line. It returns at most 100; `truncated` is true when more matched. A name that was not indexed returns no hits. Run it from the directory you indexed, or pass `--repo PATH`.

Both commands print one JSON envelope: `{"ok": …, "data": …, "error": …, "schema": 1}`. An error exits with its canon code, for example 2 when the tree has no index yet and 3 when PATH does not exist.

```
fract index .
fract find open_ledger
```

The database lives at `repos/<name>-<hash>/graph.db` under the data directory. That directory is `~/Library/Application Support/phantom-fractal` on macOS, and `$XDG_DATA_HOME/phantom-fractal` (default `~/.local/share/phantom-fractal`) elsewhere. Set `PHANTOM_FRACTAL_DATA` to use another directory. `index` refuses a data directory inside PATH.

`--version` prints `phantom-fractal 0.0.0`. `--help` lists `index` and `find` with a usage line, and so does a run with no command. `find` matches names only. Nothing resolves a name to the code that uses it, and there is no other command.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. SPDX: `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
