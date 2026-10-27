# Phantom

Phantom is a Rust code-graph engine.

Pre-release. Version 0.0.0. Nothing here is published from this tree.

The crates build on unix only. There is no Windows build.

## What works today

The `phantom-fractal` package builds two binaries from one `main.rs`: `phantom-fractal` and `fract`. Scripts should call `phantom-fractal`, since another program named `fract` may come first on `PATH`.

They have four commands:

- `index PATH` walks the Rust files under PATH, extracts their items, `use` paths, and calls, and stores them in a SQLite database in the Phantom data directory. It then builds each package's module tree and resolves calls to edges, as [Calls](#calls) describes. PATH itself is only read. A second run over an unchanged tree writes nothing.
- `find NAME` returns the indexed symbols named exactly NAME as `hits`, each with its kind, visibility, path, and line, ordered by path and then line. It returns at most 100; `truncated` is true when more matched. A name that was not indexed returns no hits.
- `callers SYMBOL` returns the resolved calls to SYMBOL.
- `callees SYMBOL` returns the resolved calls SYMBOL makes, and `unresolved_calls`: how many calls, method calls, and macros inside SYMBOL have no edge.

Run `find`, `callers`, and `callees` from the directory you indexed, or pass `--repo PATH`.

Each command prints one JSON envelope: `{"ok": …, "data": …, "error": …, "schema": 1}`. An error exits with its canon code, for example 2 when the tree has no index yet and 3 when PATH does not exist.

```
fract index .
fract find open_ledger
fract callers util::helper
fract callees crate::entry
```

The `index` envelope holds counts only, never a path:

- files: `written`, `unchanged`, `not_utf8`, and `skipped`;
- the graph: `modules`, `orphans` (`.rs` files no target reaches), and `edges`;
- `edges_by_reason`: the edges for each of the seven [reasons](#how-a-call-is-placed), keyed by reason, every key present even when it is 0;
- calls left without an edge: `unresolved`, `external`, `no_source`, `method_calls` (method calls on a receiver other than `self`), and `macros`.

A pass that rebuilds nothing reads the graph counts back from the stored graph.

The database lives at `repos/<name>-<hash>/graph.db` under the data directory. That directory is `~/Library/Application Support/phantom-fractal` on macOS, and `$XDG_DATA_HOME/phantom-fractal` (default `~/.local/share/phantom-fractal`) elsewhere. Set `PHANTOM_FRACTAL_DATA` to use another directory. `index` refuses a data directory inside PATH.

The directories Phantom creates on the way to `graph.db` are private, mode `0700`: the data directory when it is new, `repos/`, and `repos/<name>-<hash>/`. An existing `repos/` or `repos/<name>-<hash>/` that group or others can open is set back to `0700`. A data directory that already existed, such as one named by `PHANTOM_FRACTAL_DATA`, is left as it is. `graph.db`, `graph.db-wal`, and `graph.db-shm` are mode `0600`.

### What `index` skips

`index` reads regular files only. It skips:

- symlinks. A link is never followed; a target inside PATH is indexed under its own path.
- hard links: any file whose link count is over 1.
- anything else that is not a regular file, such as a fifo, socket, or device.
- files over 1 MiB. No part of such a file is stored.
- secret names: `.env*`, `*.pem`, `*.key`, `id_<name>` and `id_<name>.pub`, `credentials`, `.netrc`, `.npmrc`, `*.p12`, and `*.pfx`, ignoring ASCII case. Only the file name is matched, so `credentials.rs` and `id_codec.rs` are still indexed.

## Calls

SYMBOL is one of:

- a bare name, such as `helper`;
- a name within its file, such as `Counter::new` or `tests::check` for a function in an inline `mod tests`;
- a module-qualified name: the file's whole module path from the target root, with or without `crate::`, joined to the name within the file, such as `util::deep::sink` or `crate::util::deep::sink`. A partial path such as `deep::sink` does not match.

Output names a symbol by its qualified name from `crate`, such as `crate::tests::check`. In a workspace, `crate` is the symbol's own target, and `path` or `symbol_path` says which package it is in. A symbol in an orphan file has no module, so its name starts at its own name or `impl` type.

- No symbol matches: `not_found`, exit 3.
- More than one matches: `ambiguous`, exit 6. The message lists up to 10 candidates with their paths and lines, `data.candidates` holds the same ones, and `data.total` counts them all. Qualify the name further to pick one.
- The tree has no index, or its index predates call edges: `no_index`, exit 2. Run `index` again.

On success, `data.symbol` is the symbol's qualified name, kind, path, and line. Each entry in `data.hits` is one edge:

- `name`, `kind`, and `symbol_path`: the other symbol's qualified name, its kind, and the file it is declared in.
- `path` and `line`: the call site.
- `confidence` and `reason`: how the call was placed.

Hits are sorted by path and then line. There are at most 100, and `truncated` is true when more existed. Every path is relative to the indexed tree.

```
$ fract callers util::helper
{"ok":true,"data":{"symbol":{"name":"crate::util::helper","kind":"function","path":"src/util.rs","line":5},"hits":[{"name":"crate::entry","kind":"function","path":"src/lib.rs","line":17,"symbol_path":"src/lib.rs","confidence":1.0,"reason":"path-exact"}],"truncated":false},"error":null,"schema":1}
```

That output is cut to one hit.

### How a call is placed

A call inside a function is a candidate when it is a path call, such as `f()`, `a::f()`, `Type::new()`, or `Self::new()`, or a method call on `self`, such as `self.m()`. A single-segment name that the function binds with `let`, a parameter, a closure parameter, or a pattern is shadowed and not resolved. A call gets an edge only when exactly one symbol fits. Otherwise it gets no edge and is counted instead.

| Reason | Confidence | When |
|---|---|---|
| `path-exact` | 1.0 | The path starts with `crate`, `self`, or `super` and walks to one item or method in the same target. |
| `local` | 0.95 | The first segment is an item or child module declared in the call's module, and the rest walks to one item or method. |
| `use-alias` | 0.95 | The first segment is bound by a `use` that is not a glob, or by its `as` alias, in the call's module. The `use` path resolves by the rows above, and the rest walks to one item or method. |
| `assoc` | 0.9 | `Type::name`, where `Type` resolves by the rows above to one struct, enum, or trait, and exactly one inherent method of that type is named `name`. |
| `self-method` | 0.9 | `self.m()` in a method of an `impl` block. The impl's self type resolves by the rows above to one struct or enum, and exactly one method named `m` in the caller's target belongs to an impl of that type, inherent or trait. |
| `self-assoc` | 0.9 | `Self::m()`, exactly two segments, in a method of an `impl` block, by the same rule as `self-method`. |
| `crate-path` | 0.95 | The first segment is the crate name of a workspace lib that the caller's package may call, as below, and the rest walks from that lib's root to one item or method. The path may be written in the call or reached through a `use`. A local item, child module, or `use` with that name wins. |

So an inherent and a trait method with the same name, or two `#[cfg]` impls of one method, give `self.m()` no edge. A trait's own default body is not in an `impl` block, so its `self.m()` gets no edge either.

For `crate-path`, the crate name is the package name with `-` turned to `_`. Package A depends on workspace package B only when one of these holds:

- A's `[dependencies]` has one line `B = { … path = "…" … }` whose path, inside the indexed tree, is B's directory.
- A's `[dependencies]` has `B.workspace = true` or `B = { workspace = true … }`, and the root `Cargo.toml`'s `[workspace.dependencies]` has one line `B = { … path = "…" … }` whose path, inside the indexed tree, is B's directory.
- A is B and the call is in one of A's bins: a bin may call its own package's lib.

Any other form gives no dependency: a `[dependencies.B]` table, a version-only line, an inline table split over lines, or the forms under [Never resolved](#never-resolved). A lib that calls its own crate by name gets no edge.

Edges are rebuilt only when a `.rs` file changes. After a change to `Cargo.toml` alone, edges catch up on the next `index` that also changes a `.rs` file. Until then, `callers` and `callees` show the edges from the old manifests, while the counts a pass that rebuilds nothing prints already follow the new ones.

### Never resolved

- **Receivers:** `x.m()` for any receiver but `self`, `self.field.m()`, `(*self).m()`, and deref and autoref chains.
- **Dispatch:** a method that only a trait's provided default supplies, `<T as Trait>::f`, blanket impls, and dispatch on `dyn` or a generic.
- **Crates:** `std`, `core`, `alloc`, registry and git crates, renamed dependencies (`package = "…"`), `[target.…].dependencies`, `[dev-dependencies]`, `extern crate … as …`, and `[lib] path` and `[[bin]] path`. The targets are `src/lib.rs`, `src/main.rs`, and `src/bin/*.rs`.
- **Other edges:** edges for paths that are not calls, for types, and for struct literals, and names that only a glob import such as `use a::*` supplies.

### Nothing runs

Phantom reads source text only. Macros are never expanded: they are counted by name, and calls inside a macro's arguments are not seen. There is no type inference, so no receiver other than `self` is typed. Phantom never runs cargo, rustc, rust-analyzer, a build script, a proc macro, or any other code or command from the indexed repo.

## Other

`--version` prints `phantom-fractal 0.0.0`. `--help` lists the four commands with a usage line, and so does a run with no command. `find` matches names only. There is no other command.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. SPDX: `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
