//! Integration tests for W16 G1a `extract::resolve_query`:
//! `<crate>::` workspace aliases and `path:line` freshness enforcement.

mod common;

use common::TempDir;
use phantom_fractal_extract::{IndexError, index, resolve_query};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{Store, StoreError};

#[test]
fn resolve_query_enforces_freshness_on_path_line_and_resolves_crate_alias() {
    let dir = TempDir::new("resolve-w16");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("crates/my-crate/src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/my-crate\"]\n",
    )
    .expect("ws");
    std::fs::write(
        root.join("crates/my-crate/Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("pkg");
    std::fs::write(
        root.join("crates/my-crate/src/lib.rs"),
        "pub struct Widget;\nimpl Widget {\n    pub fn ping(&self) {}\n}\npub fn a() {} pub fn b() {}\n",
    )
    .expect("lib");

    let db = dir.path().join("graph.db");
    let mut store = Store::open(&db).expect("open");
    let lang = RustLanguage::new().expect("lang");
    index(&root, &mut store, &lang).expect("index");

    // <crate>:: alias resolves cleanly.
    let alias_hits = resolve_query(&root, &store, "my_crate::Widget::ping").expect("alias");
    assert_eq!(alias_hits.len(), 1);
    assert_eq!(alias_hits[0].qual_name, "Widget::ping");

    // Exact path:line and ./ normalization when fresh.
    let exact = resolve_query(&root, &store, "crates/my-crate/src/lib.rs:3").expect("exact");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].qual_name, "Widget::ping");

    let norm = resolve_query(&root, &store, "./crates/my-crate/src/lib.rs:3").expect("norm");
    assert_eq!(norm, exact);

    // Ambiguous 2-items-same-line.
    let amb = resolve_query(&root, &store, "crates/my-crate/src/lib.rs:5").expect("amb");
    assert_eq!(amb.len(), 2);

    // Not found when line has no item.
    assert!(
        resolve_query(&root, &store, "crates/my-crate/src/lib.rs:2")
            .expect("nf")
            .is_empty()
    );

    // Make tree stale -> path:line MUST be refused.
    std::fs::write(
        root.join("crates/my-crate/src/lib.rs"),
        "// shifted\npub struct Widget;\nimpl Widget {\n    pub fn ping(&self) {}\n}\npub fn a() {} pub fn b() {}\n",
    )
    .expect("edit");
    let err = resolve_query(&root, &store, "crates/my-crate/src/lib.rs:3").unwrap_err();
    assert!(matches!(err, IndexError::Store(StoreError::StalePathLine)));
}
