# Phantom

Phantom is a Rust code-graph engine.

Pre-release. Version 0.0.0. Nothing here is published from this tree.

The crates build on unix only. There is no Windows build.

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

The directories Phantom creates on the way to `graph.db` are private, mode `0700`: the data directory when it is new, `repos/`, and `repos/<name>-<hash>/`. An existing `repos/` or `repos/<name>-<hash>/` that group or others can open is set back to `0700`. A data directory that already existed, such as one named by `PHANTOM_FRACTAL_DATA`, is left as it is. `graph.db`, `graph.db-wal`, and `graph.db-shm` are mode `0600`.

### What `index` skips

`index` reads regular files only. It skips:

- symlinks. A link is never followed; a target inside PATH is indexed under its own path.
- hard links: any file whose link count is over 1.
- anything else that is not a regular file, such as a fifo, socket, or device.
- files over 1 MiB. No part of such a file is stored.
- secret names: `.env*`, `*.pem`, `*.key`, `id_<name>` and `id_<name>.pub`, `credentials`, `.netrc`, `.npmrc`, `*.p12`, and `*.pfx`. Only the file name is matched, so `credentials.rs` and `id_codec.rs` are still indexed.

`--version` prints `phantom-fractal 0.0.0`. `--help` lists `index` and `find` with a usage line, and so does a run with no command. `find` matches names only. Nothing resolves a name to the code that uses it, and there is no other command.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. SPDX: `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
