# Phantom

Phantom is a Rust code-graph engine.

Pre-release. Version 0.0.0. Nothing here is published from this tree.

The crates build on unix only. There is no Windows build.

## What works today

The `phantom-fractal` package builds two binaries from one `main.rs`: `phantom-fractal` and `fract`. Scripts should call `phantom-fractal`, since another program named `fract` may come first on `PATH`.

They have six commands:

- `index PATH` walks the Rust files under PATH, extracts their items, `use` paths, and calls, and stores them in a SQLite database in the Phantom data directory. It then builds each package's module tree and resolves calls to edges, as [Calls](#calls) describes. PATH itself is only read. A second run over an unchanged tree writes nothing.
- `find NAME` returns the indexed symbols named exactly NAME as `hits`, each with its kind, visibility, path, and line, ordered by path and then line. It returns at most 100; `truncated` is true when more matched. A name that was not indexed returns no hits.
- `callers SYMBOL` returns the resolved calls to SYMBOL.
- `callees SYMBOL` returns the resolved calls SYMBOL makes, and `unresolved_calls`: how many calls, method calls, and macros inside SYMBOL have no edge.
- `impact SYMBOL` walks the calls into SYMBOL backwards and says what a change to it reaches, as [Impact](#impact) describes.
- `version` prints the binary's name, version, and commit, as [Version](#version) describes. It needs no index.

Run `find`, `callers`, `callees`, and `impact` from the directory you indexed, or pass `--repo PATH`.

Each command prints one JSON envelope: `{"ok": …, "data": …, "error": …, "schema": 1}`. An error exits with its canon code, for example 2 when the tree has no index yet and 3 when PATH does not exist. Every command also accepts `--json`, before or after the command name. It changes nothing, since the output is always the envelope.

```
fract index .
fract find open_ledger
fract callers util::helper
fract callees crate::entry
fract impact util::helper --depth 2
phantom-fractal version
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
| `self-method` | 0.9 | `self.m()` in a method of an `impl` block. The impl's self type resolves by the path rows (`path-exact`, `local`, `use-alias`, and `crate-path`) to one struct or enum, so `impl Show for core_lib::Thing` counts. Exactly one method named `m` belongs to an impl of that type, inherent or trait, across the whole index, not only in the caller's target: a method of that name for the same type in another target means no edge. |
| `self-assoc` | 0.9 | `Self::m()`, exactly two segments, in a method of an `impl` block, by the same rule as `self-method`: the self type resolves by the path rows, `crate-path` included, and the method is the only candidate across the whole index. |
| `crate-path` | 0.95 | The first segment is the crate name of a workspace lib that the caller's package may call, as below, and the rest walks from that lib's root to one item or method. The path may be written in the call or reached through a `use`. A local item, child module, or `use` with that name wins. |

So an inherent and a trait method with the same name, or two `#[cfg]` impls of one method, give `self.m()` no edge. A trait's own default body is not in an `impl` block, so its `self.m()` gets no edge either.

For `crate-path`, the crate name is the package name with `-` turned to `_`. Package A depends on workspace package B only when one of these holds:

- A's `[dependencies]` has one line `B = { … path = "…" … }` whose path, inside the indexed tree, is B's directory.
- A's `[dependencies]` has `B.workspace = true` or `B = { workspace = true … }`, and the root `Cargo.toml`'s `[workspace.dependencies]` has one line `B = { … path = "…" … }` whose path, inside the indexed tree, is B's directory.
- A is B and the call is in one of A's bins: a bin may call its own package's lib.

Any other form gives no dependency: a `[dependencies.B]` table, a version-only line, an inline table split over lines, or the forms under [Never resolved](#never-resolved). A lib that calls its own crate by name gets no edge.

`index` rebuilds the crate graph and its edges when a `.rs` file is written or removed, when the index was written by another extractor version, or when the workspace fingerprint changes. The fingerprint is a blake3 hash, stored in the database, over every walked `Cargo.toml` (its path and its bytes, or that the read skipped it or found it not UTF-8) and over every `.rs` file the pass skipped or found not UTF-8 (its path and which of the two). So a `Cargo.toml`-only edit rebuilds, even one that changes only whitespace, and so does a `.rs` file that starts or stops being skipped. A pass where none of these changed writes nothing.

### Never resolved

- **Receivers:** `x.m()` for any receiver but `self`, `self.field.m()`, `(*self).m()`, and deref and autoref chains.
- **Dispatch:** a method that only a trait's provided default supplies, `<T as Trait>::f`, blanket impls, and dispatch on `dyn` or a generic.
- **Crates:** `std`, `core`, `alloc`, registry and git crates, renamed dependencies (`package = "…"`), `[target.…].dependencies`, `[dev-dependencies]`, `extern crate … as …`, and `[lib] path` and `[[bin]] path`. The targets are `src/lib.rs`, `src/main.rs`, and `src/bin/*.rs`.
- **Other edges:** edges for paths that are not calls, for types, and for struct literals, and names that only a glob import such as `use a::*` supplies.
- **Macros:** calls inside a macro invocation, such as `f(x)` in `assert!(f(x))` or `.m()` in `vec![a.m()]`, are stored as facts for [`macro_boundary`](#completeness), never as edges.

### Nothing runs

Phantom reads source text only. Macros are never expanded: they are counted by name, and a call-shaped name in a macro's tokens (`f(…)`, `a::f(…)`, or `.m(…)`, nested macros included, `macro_rules!` bodies not) is recorded as a fact with no edge. There is no type inference, so no receiver other than `self` is typed. Phantom never runs cargo, rustc, rust-analyzer, a build script, a proc macro, or any other code or command from the indexed repo.

## Impact

`impact SYMBOL` answers "what breaks if SYMBOL changes" from the stored call edges. SYMBOL is looked up as in [Calls](#calls), with the same errors: `not_found` exits 3, `ambiguous` exits 6 with `data.candidates` and `data.total`, and `no_index` exits 2. It takes `--repo` too, and:

- `--depth N`: the deepest level reported, 1 to 5. The default is 3.
- `--min-confidence F`: the lowest path confidence reported, above 0 and at most 1. The default is 0.2.
- `--no-tests`: leave tests out. `data.tests` is then absent.
- `--summary`: drop `groups` and `tests`, and keep the rest.

A value out of range, or one that is not a number, is usage and exits 1.

`data` holds `symbol` (as in [Calls](#calls)), `risk`, `exact`, `reasons`, `counts`, `groups`, `tests`, and `options`, which echoes `depth`, `min_confidence`, and `include_tests`. Every path is relative to the indexed tree.

### What is walked

- Only the stored `calls` edges, the ones `callers` lists, followed from callee to caller, breadth-first from SYMBOL. SYMBOL's callers are depth 1, their callers depth 2, and so on.
- A symbol's depth is the level where it is first reached. An edge to a symbol already placed at that level or nearer is ignored, so a cycle or a self-call never brings a symbol, or SYMBOL itself, back.
- Its `confidence` is the product of the edge confidences along its best path. SYMBOL counts as 1.0, and a symbol takes the highest product over its edges from the level before. `reason`, `site_path`, and `site_line` are that best edge's reason and call site.
- A symbol below `--min-confidence` is neither reported nor walked past. It adds 1 to `counts.below_floor`.
- `groups` has one entry per depth with a hit: `depth`, `total`, `truncated`, and at most 100 `hits`, sorted by path, then line, then name. Each hit is `name`, `kind`, `path`, `line`, `confidence`, `reason`, `site_path`, and `site_line`.

### Tests

A symbol is a test when any of these holds:

- it has `#[test]`, or an attribute whose path ends in `test`, such as `#[tokio::test]`;
- it has `#[cfg(test)]`, exactly;
- it sits inside an inline `mod` that has `#[cfg(test)]`;
- its module is declared with `#[cfg(test)]`, as in `#[cfg(test)] mod tests;`, or sits below such a module.

`#[cfg(all(test, …))]`, `#[cfg(any(test, …))]`, `#[cfg_attr(…)]`, and every other form are not recognised.

A test that is reached goes to `tests`, with its depth, and never to `groups`. It is never walked past, so no reported symbol is reached only through a test, and it never counts toward risk. With `--no-tests` it is left out altogether.

`counts` holds `direct` (non-test symbols at depth 1), `total` (every non-test symbol reported), `packages` (the distinct packages among them), `tests`, and `below_floor`.

### Completeness

`reasons` says why the answer may miss users of SYMBOL. Each entry is `{reason, count}`, in this order, and `count` is null where it cannot be counted:

| Reason | When | `count` |
|---|---|---|
| `unresolved_refs` | Calls or method calls whose last segment is SYMBOL's name have no edge. | how many |
| `macro_boundary` | Call-shaped names inside macros end in SYMBOL's name. They are facts, never edges. | how many |
| `trait_dispatch` | SYMBOL is a method of a trait impl, which dynamic or generic calls reach with no edge. | null |
| `parse_skipped_files` | The last `index` skipped `.rs` files, or its count is unknown. | the count, or null |
| `external_users_possible` | SYMBOL is `pub` in a lib target, so code outside the indexed tree may call it. | null |

`exact` is true only when `reasons` is empty. The reasons are worked out for SYMBOL alone, not for the symbols it reaches.

### Risk

`risk` is the first of these that holds:

1. `critical` when `direct` is 25 or more;
2. `unknown` when the `unresolved_refs` and `macro_boundary` counts together are at least `direct`, and at least 1;
3. `high` when `direct` is 10 or more, or `total` is 50 or more;
4. `medium` when `direct` is 3 or more, or `total` is 10 or more;
5. `low` otherwise.

```
$ fract impact report
{"ok":true,"data":{"symbol":{"name":"crate::report","kind":"function","path":"app/src/main.rs","line":16},"risk":"low","exact":true,"reasons":[],"counts":{"direct":1,"total":2,"packages":1,"tests":0,"below_floor":0},"groups":[{"depth":1,"total":1,"truncated":false,"hits":[{"name":"crate::run","kind":"function","path":"app/src/main.rs","line":9,"confidence":0.95,"reason":"local","site_path":"app/src/main.rs","site_line":13}]}],"tests":[],"options":{"depth":3,"min_confidence":0.2,"include_tests":true}},"error":null,"schema":1}
```

That output, from `fixtures/impact`, is cut to one hit: the depth-2 group is left out.

### What impact does not do

Canon §7.3 describes more than this command does. There is no impact of a diff, no file path as a target, no path between two symbols, no `--edges`, no rank, no co-change, and no token budget. Only `calls` edges are walked, so receivers other than `self` and trait dispatch are not followed.

## Version

`phantom-fractal version` prints one line and exits 0:

```
{"ok":true,"data":{"name":"phantom-fractal","version":"0.0.0","git_sha":"<40 lowercase hex, or null>","extractor_version":5,"store_schema":3},"error":null,"schema":1}
```

`phantom-fractal --json version`, `phantom-fractal version --json`, and `fract version` print the same line. `data` holds these fields, in this order:

| Field | Value |
|---|---|
| `name` | `phantom-fractal`, under `fract` too |
| `version` | the CLI crate's version, `0.0.0` |
| `git_sha` | the commit the binary was built from, as 40 lowercase hex characters, or `null` when unknown |
| `extractor_version` | the extractor version the binary writes, 5 |
| `store_schema` | the store schema version the binary writes, 3 |

`git_sha` is fixed at build time. `PHANTOM_FRACTAL_GIT_SHA`, when it holds 40 hex characters, sets it, for packagers. Otherwise it is the output of `git rev-parse HEAD` in the CLI crate's directory. With no git, no `.git` (as in a crates.io tarball), or a refused repository, it is `null`, and the build still succeeds.

`version` never opens a store, reads the data directory, walks a tree, or reads `--repo`. It works in any directory, with no index and with `HOME` unset. An extra argument is usage and exits 1.

The envelope keys come in the order `ok`, `data`, `error`, `schema`. `schema` is the version of the envelope itself, 1 today, the same number every command prints. A consumer reads that integer and refuses a value it does not know. More fields may be added to `data` later, and a consumer ignores keys it does not know.

`--version` and `-V` still print the plain line `phantom-fractal 0.0.0`.

## Other

`--version` prints `phantom-fractal 0.0.0`. `--help` lists the six commands with a usage line, and so does a run with no command. `find` matches names only. There is no other command.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option. SPDX: `MIT OR Apache-2.0`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 licence, shall be dual licensed as above, without any additional terms or conditions.
