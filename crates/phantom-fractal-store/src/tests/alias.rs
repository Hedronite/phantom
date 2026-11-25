//! W16 G1a store tests: `<crate>::` alias resolution, exact `path:line`
//! lookup with `IndexState::Stale` refusal, and `atomic_batch` rollback.

use phantom_fractal_lang::{
    Edge, Link, LinkKind, LinkReason, Module, Node, Reason, Target, TargetKind,
};

use super::graph::{language, put};
use super::{TempDb, assert_outside_workspace};
use crate::{AliasRewrite, CochangeScan, IndexState, Package, Store, StoreError, parse_path_line};

fn two_crate_store(label: &str) -> (TempDb, Store) {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    put(
        &mut store,
        &lang,
        "crates/lapis-lattice/src/lib.rs",
        "pub mod sub;\n\npub fn now_ms() -> u64 { 1 }\npub fn shared() {}\n\npub struct Ledger;\nimpl Ledger {\n    pub fn open(&self) {}\n}\n",
    );
    put(
        &mut store,
        &lang,
        "crates/lapis-lattice/src/sub.rs",
        "pub fn helper() {}\npub struct Sub;\nimpl Sub {\n    pub fn run(&self) {}\n}\n",
    );
    put(
        &mut store,
        &lang,
        "crates/app/src/lib.rs",
        "pub fn is_root() -> bool { true }\npub fn shared() {}\n",
    );
    put(
        &mut store,
        &lang,
        "crates/app/src/bin/tool.rs",
        "pub fn cli_entry() {}\n",
    );
    store
        .write_crate_graph(
            &[
                Package {
                    name: "lapis-lattice".to_owned(),
                    version: None,
                    manifest_path: "crates/lapis-lattice/Cargo.toml".to_owned(),
                },
                Package {
                    name: "app".to_owned(),
                    version: None,
                    manifest_path: "crates/app/Cargo.toml".to_owned(),
                },
            ],
            &[
                Target {
                    id: 1,
                    package: "lapis-lattice".to_owned(),
                    name: "lapis-lattice".to_owned(),
                    kind: TargetKind::Lib,
                    root: "crates/lapis-lattice/src/lib.rs".to_owned(),
                },
                Target {
                    id: 2,
                    package: "app".to_owned(),
                    name: "app".to_owned(),
                    kind: TargetKind::Lib,
                    root: "crates/app/src/lib.rs".to_owned(),
                },
                Target {
                    id: 3,
                    package: "app".to_owned(),
                    name: "tool".to_owned(),
                    kind: TargetKind::Bin,
                    root: "crates/app/src/bin/tool.rs".to_owned(),
                },
            ],
            &[
                Module {
                    id: 10,
                    target: 1,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "crates/lapis-lattice/src/lib.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 11,
                    target: 1,
                    parent: Some(10),
                    path: "crate::sub".to_owned(),
                    file: "crates/lapis-lattice/src/sub.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 20,
                    target: 2,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "crates/app/src/lib.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 30,
                    target: 3,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "crates/app/src/bin/tool.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            &[],
        )
        .expect("graph");
    (db, store)
}

#[test]
fn crate_prefix_aliases_resolve_scoped_and_normalize_hyphens() {
    let (_db, store) = two_crate_store("crate-alias");

    for query in ["lapis_lattice::now_ms", "lapis-lattice::now_ms"] {
        let hits = store.symbols_matching(query).expect(query);
        assert_eq!(hits.len(), 1, "{query}: {hits:?}");
        assert_eq!(hits[0].file, "crates/lapis-lattice/src/lib.rs");
        assert_eq!(hits[0].qual_name, "now_ms");
    }

    for query in ["lapis_lattice::Ledger::open", "lapis-lattice::Ledger::open"] {
        let hits = store.symbols_matching(query).expect(query);
        assert_eq!(hits.len(), 1, "{query}: {hits:?}");
        assert_eq!(hits[0].qual_name, "Ledger::open");
    }

    let sub_fn = store
        .symbols_matching("lapis_lattice::sub::helper")
        .expect("sub fn");
    assert_eq!(sub_fn.len(), 1, "{sub_fn:?}");
    assert_eq!(sub_fn[0].file, "crates/lapis-lattice/src/sub.rs");

    let sub_method = store
        .symbols_matching("lapis_lattice::sub::run")
        .expect("sub method");
    assert_eq!(sub_method.len(), 1, "{sub_method:?}");
    assert_eq!(sub_method[0].qual_name, "Sub::run");

    let app_root = store.symbols_matching("app::is_root").expect("app");
    assert_eq!(app_root.len(), 1, "{app_root:?}");
    assert_eq!(app_root[0].file, "crates/app/src/lib.rs");

    // Distinct bin target resolves only its own target symbols.
    let tool = store.symbols_matching("tool::cli_entry").expect("tool");
    assert_eq!(tool.len(), 1, "{tool:?}");
    assert_eq!(tool[0].file, "crates/app/src/bin/tool.rs");
    assert!(
        store
            .symbols_matching("tool::is_root")
            .expect("tool must not see lib")
            .is_empty()
    );

    // Multi-crate isolation: shared symbol in crate A never leaks crate B.
    let only_lapis = store
        .symbols_matching("lapis_lattice::shared")
        .expect("lapis shared");
    assert_eq!(only_lapis.len(), 1, "{only_lapis:?}");
    assert_eq!(only_lapis[0].file, "crates/lapis-lattice/src/lib.rs");

    let only_app = store.symbols_matching("app::shared").expect("app shared");
    assert_eq!(only_app.len(), 1, "{only_app:?}");
    assert_eq!(only_app[0].file, "crates/app/src/lib.rs");

    // D3 preserved: bare `shared` returns both.
    assert_eq!(store.symbols_matching("shared").expect("bare").len(), 2);

    // Unknown crate prefix never silent-heals.
    assert!(
        store
            .symbols_matching("ghost_crate::now_ms")
            .expect("unknown crate")
            .is_empty()
    );
    for noop in ["lapis_lattice::", "self::now_ms", "super::now_ms"] {
        assert!(store.symbols_matching(noop).expect(noop).is_empty());
    }

    // Helper APIs for G1b / Go seam.
    let aliases = store.package_aliases().expect("aliases");
    assert_eq!(aliases.len(), 3);
    assert_eq!(
        store
            .rewrite_crate_alias("lapis_lattice::now_ms")
            .expect("rewrite"),
        Some(AliasRewrite {
            prefix: "lapis_lattice".to_owned(),
            package: "lapis-lattice".to_owned(),
            target: "lapis-lattice".to_owned(),
            rewritten: "crate::now_ms".to_owned(),
        })
    );
    for no_rewrite in [
        "ghost_crate::now_ms",
        "lapis_lattice::",
        "crate::now_ms",
        "self::now_ms",
        "super::now_ms",
    ] {
        assert_eq!(
            store.rewrite_crate_alias(no_rewrite).expect(no_rewrite),
            None,
            "{no_rewrite}"
        );
    }
}

#[test]
fn base_module_named_like_a_crate_keeps_d3_rows() {
    let db = TempDb::new("d3-mod-collision");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    put(
        &mut store,
        &lang,
        "src/lib.rs",
        "pub mod sample;\npub fn root_only() {}\n",
    );
    put(&mut store, &lang, "src/sample.rs", "pub fn inner() {}\n");
    store
        .write_crate_graph(
            &[Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "Cargo.toml".to_owned(),
            }],
            &[Target {
                id: 1,
                package: "sample".to_owned(),
                name: "sample".to_owned(),
                kind: TargetKind::Lib,
                root: "src/lib.rs".to_owned(),
            }],
            &[
                Module {
                    id: 1,
                    target: 1,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "src/lib.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 2,
                    target: 1,
                    parent: Some(1),
                    path: "crate::sample".to_owned(),
                    file: "src/sample.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            &[],
        )
        .expect("graph");

    // Base answers `sample::inner` directly without hitting alias rewrite.
    let hits = store.symbols_matching("sample::inner").expect("base");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].file, "src/sample.rs");
    // Crate alias still resolves `sample::root_only` when base misses.
    let root_hits = store
        .symbols_matching("sample::root_only")
        .expect("alias fallback");
    assert_eq!(root_hits.len(), 1);
    assert_eq!(root_hits[0].file, "src/lib.rs");
}

#[test]
fn exact_path_line_lookup_enforces_freshness_and_ambiguity() {
    let db = TempDb::new("path-line");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    let src = "pub fn first() {\n    let x = 1;\n}\npub fn a() {} pub fn b() {}\n";
    put(&mut store, &lang, "src/foo.rs", src);

    // Parsing edge cases.
    assert!(parse_path_line("src/foo.rs:1").is_some());
    assert_eq!(
        parse_path_line("././src/foo.rs:4").map(|p| (p.path, p.line)),
        Some(("src/foo.rs", 4))
    );
    for invalid in [
        "src/foo.rs:0",
        "src/foo.rs:-1",
        "src/foo.rs:abc",
        "src/foo.rs:",
        "src/foo.rs::1",
        "a::b:1",
        "foo:1",
        "./:1",
        "src/foo.rs:999999999999999999999999",
    ] {
        assert!(parse_path_line(invalid).is_none(), "{invalid}");
    }

    // Stale index MUST refuse path:line.
    let stale_err = store
        .symbols_matching_for("src/foo.rs:1", IndexState::Stale)
        .unwrap_err();
    assert!(matches!(stale_err, StoreError::StalePathLine));
    assert!(stale_err.to_string().contains("stale"));
    assert!(matches!(
        store.symbols_matching("src/foo.rs:1"),
        Err(StoreError::StalePathLine)
    ));

    // Fresh exact match and `./` normalization.
    let exact = store
        .symbols_matching_for("src/foo.rs:1", IndexState::Fresh)
        .expect("exact");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].qual_name, "first");

    let normalized = store
        .symbols_matching_for("./src/foo.rs:1", IndexState::Fresh)
        .expect("normalized");
    assert_eq!(normalized, exact);

    // No fuzzy nearest-line guessing (line 2 is inside `first`, not its start_line).
    assert!(
        store
            .symbols_matching_for("src/foo.rs:2", IndexState::Fresh)
            .expect("exact line only")
            .is_empty()
    );

    // 2 items on the same line return both so lookup reports ambiguous.
    let same_line = store
        .symbols_matching_for("src/foo.rs:4", IndexState::Fresh)
        .expect("same line");
    assert_eq!(same_line.len(), 2, "{same_line:?}");

    // Unknown file -> empty (not_found).
    assert!(
        store
            .symbols_matching_for("src/missing.rs:1", IndexState::Fresh)
            .expect("missing")
            .is_empty()
    );

    // did_you_mean for missed path:line offers nearest symbols in that file.
    let dym = store.did_you_mean("src/foo.rs:2").expect("dym");
    assert!(!dym.is_empty());
    assert_eq!(dym[0].qual_name, "first");
    assert!(
        store
            .did_you_mean("src/missing.rs:1")
            .expect("none")
            .is_empty()
    );

    // Non-path:line query still resolves under IndexState::Stale.
    assert_eq!(
        store
            .symbols_matching_for("first", IndexState::Stale)
            .expect("name")
            .len(),
        1
    );
}

#[test]
fn atomic_batch_commits_and_rolls_back_cleanly() {
    let db = TempDb::new("atomic-batch");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    put(&mut store, &lang, "src/base.rs", "pub fn base() {}\n");

    // Rolled-back error leaves zero traces of partial puts/removes.
    let err: Result<(), StoreError> = store.atomic_batch(|s| {
        s.put_source("src/new.rs", "pub fn added() {}\n", 0, &lang)?;
        s.remove_file("src/base.rs")?;
        Err(StoreError::UnknownTable)
    });
    assert!(matches!(err, Err(StoreError::UnknownTable)));
    assert_eq!(store.file_paths().expect("paths"), vec!["src/base.rs"]);

    // Nested atomic_batch: inner error rolls back inner savepoint while outer commits.
    store
        .atomic_batch(|s| {
            s.put_source("src/outer.rs", "pub fn outer() {}\n", 0, &lang)?;
            let inner: Result<(), StoreError> = s.atomic_batch(|inner_s| {
                inner_s.put_source("src/inner.rs", "pub fn inner() {}\n", 0, &lang)?;
                Err(StoreError::UnknownTable)
            });
            assert!(inner.is_err());
            s.atomic_batch(|inner_s| {
                inner_s.write_crate_graph(&[], &[], &[], &[])?;
                inner_s.replace_edges(&[])?;
                inner_s.write_links(&[])?;
                inner_s.write_method_owners(&[])?;
                inner_s.write_rank(1, &[], &[])?;
                inner_s.write_cochange(&CochangeScan::default())?;
                inner_s.set_workspace_fingerprint(&[1u8; 32], 0)?;
                Ok::<(), StoreError>(())
            })
        })
        .expect("outer commit");
    assert_eq!(
        store.file_paths().expect("after nested"),
        vec!["src/base.rs", "src/outer.rs"]
    );
    // Verify commit_batch actually committed to disk (visible from another connection,
    // and starting a new outer BEGIN IMMEDIATE succeeds).
    let reopened = Store::open(db.path()).expect("reopen");
    assert_eq!(
        reopened.file_paths().expect("reopened paths"),
        vec!["src/base.rs", "src/outer.rs"]
    );
    drop(reopened);
    store
        .atomic_batch(|s| s.put_source("src/second.rs", "pub fn second() {}\n", 0, &lang))
        .expect("second outer batch");
    assert_eq!(
        Store::open(db.path())
            .expect("reopen2")
            .file_paths()
            .expect("paths2"),
        vec!["src/base.rs", "src/outer.rs", "src/second.rs"]
    );
    // Keep unused imports exercised cleanly for link/edge types.
    let _ = (
        Edge {
            ref_id: 0,
            source: 0,
            target: 0,
            confidence: 1.0,
            reason: Reason::Local,
        },
        Link {
            kind: LinkKind::Import,
            source: Node::Symbol(0),
            target: Node::Symbol(0),
            confidence: 1.0,
            reason: LinkReason::Local,
            ref_id: None,
        },
    );
}

#[test]
#[should_panic(expected = "simulated mid-batch panic")]
fn atomic_batch_rolls_back_on_panic() {
    let db = TempDb::new("atomic-panic");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store.atomic_batch::<(), StoreError>(|s| {
            s.put_source("src/lost.rs", "pub fn lost() {}\n", 0, &lang)?;
            panic!("simulated mid-batch panic");
        });
    }));
    assert!(result.is_err());
    assert!(store.file_paths().expect("paths").is_empty());
    std::panic::resume_unwind(result.unwrap_err());
}

#[test]
fn crate_alias_returns_sorted_unique_hits_across_multiple_scopes() {
    let db = TempDb::new("multi-scope-alias");
    let mut store = Store::open(db.path()).expect("open");
    let lang = language();
    put(
        &mut store,
        &lang,
        "crates/pkg1/src/a_cli.rs",
        "pub fn entry() {}\n",
    );
    put(
        &mut store,
        &lang,
        "crates/pkg2/src/z_lib.rs",
        "pub fn entry() {}\n",
    );
    store
        .write_crate_graph(
            &[
                Package {
                    name: "host-pkg".to_owned(),
                    version: None,
                    manifest_path: "crates/pkg1/Cargo.toml".to_owned(),
                },
                Package {
                    name: "my_cli".to_owned(),
                    version: None,
                    manifest_path: "crates/pkg2/Cargo.toml".to_owned(),
                },
            ],
            &[
                Target {
                    id: 1,
                    package: "host-pkg".to_owned(),
                    name: "my-cli".to_owned(),
                    kind: TargetKind::Bin,
                    root: "crates/pkg1/src/a_cli.rs".to_owned(),
                },
                Target {
                    id: 2,
                    package: "my_cli".to_owned(),
                    name: "my_cli".to_owned(),
                    kind: TargetKind::Lib,
                    root: "crates/pkg2/src/z_lib.rs".to_owned(),
                },
            ],
            &[
                Module {
                    id: 10,
                    target: 1,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "crates/pkg1/src/a_cli.rs".to_owned(),
                    scope: String::new(),
                },
                Module {
                    id: 20,
                    target: 2,
                    parent: None,
                    path: "crate".to_owned(),
                    file: "crates/pkg2/src/z_lib.rs".to_owned(),
                    scope: String::new(),
                },
            ],
            &[],
        )
        .expect("graph");
    // Package scope (`my_cli`, `z_lib.rs`) is collected before Target scope
    // (`my-cli`, `a_cli.rs`), so `sort_symbol_matches` is required to order them.
    let hits = store.symbols_matching("my_cli::entry").expect("multi");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].file, "crates/pkg1/src/a_cli.rs");
    assert_eq!(hits[1].file, "crates/pkg2/src/z_lib.rs");
}
