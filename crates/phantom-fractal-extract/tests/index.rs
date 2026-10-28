//! Index passes over the fixture into a temp store (SPEC-index G1b).

mod common;

use common::{FixtureRepo, TempDir};
use phantom_fractal_extract::index;
use phantom_fractal_lang::{Item, ItemKind, Visibility};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const RUST_FILES: [&str; 8] = [
    "credentials.rs",
    "edge.rs",
    "netrc.rs",
    "npmrc.rs",
    "src/id_codec.rs",
    "src/ledger.rs",
    "src/lib.rs",
    "src/target/mod.rs",
];

fn sorted(paths: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = paths.iter().map(String::as_str).collect();
    out.sort_unstable();
    out
}

struct Setup {
    repo: FixtureRepo,
    _db_dir: TempDir,
    store: Store,
    rust: RustLanguage,
}

fn setup() -> Setup {
    let repo = FixtureRepo::new();
    let db_dir = TempDir::new("db");
    let store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    Setup {
        repo,
        _db_dir: db_dir,
        store,
        rust,
    }
}

#[test]
fn first_pass_writes_each_kept_rust_file() {
    let mut s = setup();
    let report = index(s.repo.root(), &mut s.store, &s.rust).expect("index");
    assert_eq!(sorted(&report.written), RUST_FILES);
    assert!(report.unchanged.is_empty());
    assert!(report.not_utf8.is_empty());
    assert_eq!(s.store.row_count("files").expect("count"), 8);
}

#[test]
fn id_codec_is_not_a_key_and_its_symbol_is_stored() {
    let mut s = setup();
    index(s.repo.root(), &mut s.store, &s.rust).expect("index");
    let decode_id = Item {
        kind: ItemKind::Function,
        name: "decode_id".to_owned(),
        visibility: Visibility::Public,
        start_line: 3,
        is_test: false,
    };
    let items = s.store.read_items("src/id_codec.rs").expect("read");
    assert!(items.contains(&decode_id), "{items:?}");
    assert!(s.store.match_count("decode_id").expect("match") >= 1);
}

#[test]
fn stored_items_use_repo_relative_paths() {
    let mut s = setup();
    index(s.repo.root(), &mut s.store, &s.rust).expect("index");
    let items = s.store.read_items("src/ledger.rs").expect("read");
    let open_ledger = Item {
        kind: ItemKind::Function,
        name: "open_ledger".to_owned(),
        visibility: Visibility::Public,
        start_line: 7,
        is_test: false,
    };
    assert!(items.contains(&open_ledger), "{items:?}");
    let absolute = s.repo.root().join("src/ledger.rs");
    let absolute = absolute.to_str().expect("utf-8 temp path");
    assert!(s.store.read_items(absolute).expect("read").is_empty());
}

#[test]
fn second_pass_with_same_hashes_writes_nothing() {
    let mut s = setup();
    index(s.repo.root(), &mut s.store, &s.rust).expect("first index");
    let files = s.store.row_count("files").expect("count");
    let symbols = s.store.row_count("symbols").expect("count");
    let report = index(s.repo.root(), &mut s.store, &s.rust).expect("second index");
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert_eq!(sorted(&report.unchanged), RUST_FILES);
    assert_eq!(s.store.row_count("files").expect("count"), files);
    assert_eq!(s.store.row_count("symbols").expect("count"), symbols);
}

#[test]
fn passes_leave_the_tree_and_its_git_dir_byte_identical() {
    let mut s = setup();
    let before = s.repo.snapshot();
    assert!(before.keys().any(|path| path.starts_with(".git")));
    index(s.repo.root(), &mut s.store, &s.rust).expect("first index");
    index(s.repo.root(), &mut s.store, &s.rust).expect("second index");
    assert_eq!(s.repo.snapshot(), before);
}

#[test]
fn skipped_files_are_reported_and_not_stored() {
    let mut s = setup();
    let report = index(s.repo.root(), &mut s.store, &s.rust).expect("index");
    for path in [
        "big.rs",
        "link_in.rs",
        "link_out.rs",
        "hard_out.rs",
        "credentials",
        "keys/id_ed25519",
    ] {
        assert!(
            report.skipped.iter().any(|skip| skip.path == path),
            "{path}"
        );
        assert!(s.store.read_items(path).expect("read").is_empty(), "{path}");
    }
}
