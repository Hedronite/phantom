//! Crate-level integration tests for Go extraction, build-excluded metadata,
//! mixed Rust+Go resolution, and multi-manifest graphs inside `phantom-fractal-extract`.

mod common;

use std::path::Path;

use common::{TempDir, copy_tree, write};
use phantom_fractal_extract::{default_registry, index_with, stored_graph_with};
use phantom_fractal_store::Store;

#[test]
fn go_fixtures_build_excluded_and_mixed_rust_go_extract_cleanly() {
    let registry = default_registry().expect("registry");

    // 1. fixtures/go-testpkg: persists build_excluded_count=2 and per-module excluded terms.
    let tp_dir = TempDir::new("gx-testpkg");
    let fixture = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/go-testpkg"
    ));
    copy_tree(fixture, tp_dir.path());
    let db_dir = TempDir::new("gx-db");
    let mut store = Store::open(&db_dir.path().join("g.db")).expect("open");

    let rep = index_with(tp_dir.path(), &mut store, &registry).expect("index go-testpkg");
    let graph = rep.graph.expect("graph");
    assert_eq!(graph.orphans, 0);
    assert_eq!(store.build_excluded_count().expect("count"), Some(2));
    let platform_matches = store.symbols_matching("Platform").expect("Platform");
    assert_eq!(platform_matches.len(), 1);
    let platform_target = store
        .symbol_facts(platform_matches[0].id)
        .expect("facts query")
        .expect("facts");
    assert_eq!(
        store
            .build_excluded_for_targets(&[platform_target])
            .expect("terms match"),
        Some(2)
    );
    let stored = stored_graph_with(&store, tp_dir.path(), &registry).expect("stored");
    assert_eq!(stored.counts, graph.counts);
    assert_eq!(stored.orphans, 0);

    // Removing Go files/manifest and indexing Rust-only clears build_excluded_count to Some(0).
    for rel in [
        "go.mod",
        "internal/calc/calc.go",
        "internal/calc/calc_external_test.go",
        "internal/calc/calc_internal_test.go",
        "internal/calc/impl_darwin.go",
        "internal/calc/impl_linux.go",
        "internal/calc/tagged.go",
    ] {
        let _ = std::fs::remove_file(tp_dir.path().join(rel));
    }
    write(
        tp_dir.path(),
        "Cargo.toml",
        b"[package]\nname = \"onlyrs\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        tp_dir.path(),
        "src/lib.rs",
        b"pub fn a() { b(); }\npub fn b() {}\n",
    );
    let rs_rep = index_with(tp_dir.path(), &mut store, &registry).expect("index rust");
    assert!(rs_rep.graph.is_some());
    assert_eq!(store.build_excluded_count().expect("cleared"), Some(0));

    // 2. Fresh Rust-only repo (with no Cargo.toml) keeps Rust active via extension and leaves build_excluded None.
    let rs_only_dir = TempDir::new("gx-rsonly");
    write(
        rs_only_dir.path(),
        "src/lib.rs",
        b"pub fn top() { sub(); }\npub fn sub() {}\n",
    );
    let mut rs_only_store = Store::open(&db_dir.path().join("rsonly.db")).expect("open");
    let ro_rep = index_with(rs_only_dir.path(), &mut rs_only_store, &registry).expect("index");
    let ro_g = ro_rep.graph.expect("graph");
    assert_eq!(ro_g.orphans, 1);
    assert_eq!(ro_g.counts.unresolved, 1);
    assert_eq!(rs_only_store.build_excluded_count().expect("none"), None);

    // 3. Empty go.mod / go.work-only repos trigger filtered driver tracking with count Some(0).
    let mod_only = TempDir::new("gx-gomod-only");
    write(
        mod_only.path(),
        "go.mod",
        b"module example.com/empty\ngo 1.26\n",
    );
    let mut mod_store = Store::open(&db_dir.path().join("modonly.db")).expect("open");
    let mod_rep = index_with(mod_only.path(), &mut mod_store, &registry).expect("index");
    assert!(mod_rep.graph.is_some());
    assert_eq!(mod_store.build_excluded_count().expect("zero"), Some(0));
    assert!(
        mod_store
            .canonical_dump()
            .expect("dump")
            .contains("example.com/empty")
    );

    let work_only = TempDir::new("gx-gowork-only");
    write(work_only.path(), "go.work", b"go 1.26\n");
    let mut work_store = Store::open(&db_dir.path().join("workonly.db")).expect("open");
    index_with(work_only.path(), &mut work_store, &registry).expect("index");
    assert_eq!(work_store.build_excluded_count().expect("zero"), Some(0));

    // 4. fixtures/go-embed-iface + mixed Rust+Go resolve method owners and cross-calls cleanly.
    let emb_dir = TempDir::new("gx-embed");
    copy_tree(
        Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/go-embed-iface"
        )),
        emb_dir.path(),
    );
    write(
        emb_dir.path(),
        "Cargo.toml",
        b"[package]\nname = \"mix\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        emb_dir.path(),
        "src/lib.rs",
        b"pub fn rs_top() { rs_leaf(); }\npub fn rs_leaf() {}\n",
    );
    let mut mix_store = Store::open(&db_dir.path().join("mix.db")).expect("open");
    let mix_rep = index_with(emb_dir.path(), &mut mix_store, &registry).expect("index mix");
    let mix_g = mix_rep.graph.expect("mix graph");
    assert_eq!(mix_g.orphans, 0);
    assert!(mix_g.counts.edges >= 2);
    let rs_leaf = mix_store.symbols_matching("rs_leaf").expect("rs_leaf");
    assert_eq!(rs_leaf.len(), 1);
    assert_eq!(mix_store.callers(rs_leaf[0].id).expect("callers").len(), 1);
}
