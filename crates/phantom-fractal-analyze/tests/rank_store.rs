//! Rank read through [`Store`] after indexing `fixtures/graph`
//! (SPEC-w7 G1b).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{EdgeKind, RankSource, personalised, rank};
use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/graph");
const WORKSPACE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/workspace");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-analyze-{tag}-{}-{nanos}",
            process::id()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `fixtures/graph` indexed into a fresh store.
fn indexed() -> (TempDir, Store) {
    indexed_at(FIXTURE)
}

/// `fixture` indexed into a fresh store. The fixture is only read.
fn indexed_at(fixture: &str) -> (TempDir, Store) {
    let db = TempDir::new("rank");
    let mut store = Store::open(&db.0.join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    index(Path::new(fixture), &mut store, &rust).expect("index");
    (db, store)
}

/// The database file and its write-ahead log.
fn db_bytes(db: &TempDir) -> Vec<Vec<u8>> {
    ["graph.db", "graph.db-wal"]
        .iter()
        .map(|name| match fs::read(db.0.join(name)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => Vec::new(),
            Err(err) => panic!("read {name}: {err}"),
        })
        .collect()
}

fn paths(store: &Store) -> BTreeMap<i64, String> {
    store
        .files()
        .expect("files")
        .into_iter()
        .map(|file| (file.id, file.path))
        .collect()
}

#[test]
fn the_store_feeds_every_symbol_edge_and_import_of_the_fixture() {
    let (_db, store) = indexed();
    let nodes = store.nodes().expect("nodes");
    let ids: Vec<i64> = store
        .rank_input()
        .expect("rank input")
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(nodes.iter().map(|node| node.id).collect::<Vec<_>>(), ids);
    // The six impl links of the fixture's EXPECTED.md, symbol to symbol.
    let edges = store.edges().expect("edges");
    let impls = edges.iter().filter(|edge| edge.kind == EdgeKind::Impl);
    assert_eq!(impls.count(), 6);
    assert!(edges.iter().any(|edge| edge.kind == EdgeKind::Call));
    // Every import link as a pair of files, each pair once.
    let paths = paths(&store);
    let pairs: BTreeSet<(&str, &str)> = store
        .imports()
        .expect("imports")
        .iter()
        .map(|(from, to)| (paths[from].as_str(), paths[to].as_str()))
        .collect();
    let want: BTreeSet<(&str, &str)> = [
        ("app/src/shapes.rs", "app/src/traits.rs"),
        ("app/src/shapes.rs", "kit/src/lib.rs"),
        ("app/src/uses.rs", "app/src/shapes.rs"),
        ("app/src/uses.rs", "app/src/uses.rs"),
        ("app/src/uses.rs", "kit/src/geo.rs"),
        ("app/src/uses.rs", "kit/src/lib.rs"),
    ]
    .into_iter()
    .collect();
    assert_eq!(pairs, want);
    assert_eq!(store.imports().expect("imports").len(), 12);
}

#[test]
fn rank_reads_never_write_and_repeat_exactly() {
    let (db, store) = indexed();
    let before = db_bytes(&db);
    let first = rank(&store).expect("rank");
    assert_eq!(rank(&store).expect("rank"), first);
    assert_eq!(db_bytes(&db), before);
    assert!(first.epoch > 0);
    assert_eq!(first.epoch, store.graph_epoch().expect("epoch"));
    for list in [&first.symbols, &first.files] {
        let sum: f64 = list.iter().map(|(_, score)| score).sum();
        assert!((sum - 1.0).abs() < 1e-9, "sum {sum}");
    }
    // Every file with a symbol or an import has a score.
    let paths = paths(&store);
    let scored: Vec<&str> = first
        .files
        .iter()
        .map(|(id, _)| paths[id].as_str())
        .collect();
    for path in ["app/src/shapes.rs", "app/src/uses.rs", "kit/src/lib.rs"] {
        assert!(scored.contains(&path), "{path}");
    }
}

#[test]
fn personalised_rank_lifts_the_focus_file() {
    let (_db, store) = indexed();
    let id = |path: &str| {
        paths(&store)
            .into_iter()
            .find(|(_, p)| p == path)
            .map(|(id, _)| id)
            .expect("file")
    };
    let traits = id("app/src/traits.rs");
    let global = rank(&store).expect("rank");
    let focused = personalised(&store, &[traits]).expect("personalised");
    let score = |ranks: &[(i64, f64)]| {
        ranks
            .iter()
            .find(|(file, _)| *file == traits)
            .map(|(_, score)| *score)
            .expect("score")
    };
    assert!(score(&focused.files) > score(&global.files));
    let top = focused
        .files
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("top");
    assert_eq!(top.0, traits);
}

/// Whether the prior boosts each named symbol, as `path name`.
fn boosts(store: &Store) -> BTreeMap<String, bool> {
    let paths = paths(store);
    let names: BTreeMap<i64, String> = store
        .map_rows()
        .expect("map rows")
        .into_iter()
        .map(|row| (row.id, row.name))
        .collect();
    store
        .nodes()
        .expect("nodes")
        .into_iter()
        .filter_map(|node| {
            let name = names.get(&node.id)?;
            Some((format!("{} {name}", paths[&node.file_id]), node.boosted))
        })
        .collect()
}

#[test]
fn the_prior_boosts_public_lib_items_and_a_bins_main() {
    let (_db, store) = indexed_at(WORKSPACE);
    let boosted = boosts(&store);
    assert!(boosted["app/src/main.rs main"]);
    assert!(boosted["app/src/lib.rs run"]);
    let (_db, store) = indexed();
    let boosted = boosts(&store);
    assert!(boosted["kit/src/lib.rs scale"]);
    assert!(boosted["app/src/shapes.rs new"]);
    // A trait impl's method has no `pub`.
    assert!(!boosted["app/src/shapes.rs describe"]);
    assert!(!boosted["app/src/shapes.rs sides"]);
}

#[test]
fn a_public_item_of_a_bin_is_not_boosted() {
    let repo = TempDir::new("bin");
    fs::create_dir_all(repo.0.join("src")).expect("src");
    fs::write(
        repo.0.join("Cargo.toml"),
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\n",
    )
    .expect("manifest");
    fs::write(
        repo.0.join("src/main.rs"),
        "pub fn helper() {}\n\nfn main() {\n    helper();\n}\n",
    )
    .expect("main");
    let (_db, store) = indexed_at(repo.0.to_str().expect("utf-8 temp path"));
    let boosted = boosts(&store);
    assert!(boosted["src/main.rs main"]);
    assert!(!boosted["src/main.rs helper"]);
}
