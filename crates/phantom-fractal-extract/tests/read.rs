//! The bounded, no-follow read (SPEC-w1 G0, SPEC-w2 G0a).

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

use common::{
    BIG_HEAD, EDGE_HEAD, FixtureRepo, HARD_OUT_TEXT, TempDir, rust_bytes, without_detail, write,
};
use phantom_fractal_extract::{
    IndexReport, MAX_FILE_BYTES, SkipReason, Skipped, Source, index, read_source,
};
use phantom_fractal_lang::{Item, ItemKind, Visibility};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const OUTSIDE_TEXT: &[u8] = b"pub fn outside_root() {}\n";

fn make_fifo(path: &Path) {
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Indexes `root` into a fresh store in its own temp dir.
fn index_into_store(root: &Path) -> (TempDir, Store, IndexReport) {
    let db_dir = TempDir::new("db");
    let mut store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    let report = index(root, &mut store, &rust).expect("index");
    (db_dir, store, report)
}

#[test]
fn open_refuses_a_symlink_to_an_outside_file() {
    let root = TempDir::new("read");
    let outside = TempDir::new("outside");
    write(outside.path(), "elsewhere.rs", OUTSIDE_TEXT);
    let link = root.path().join("link_out.rs");
    symlink(outside.path().join("elsewhere.rs"), &link).expect("link out");
    let link_len = fs::symlink_metadata(&link).expect("lstat").len();
    assert!(link_len < MAX_FILE_BYTES, "{link_len}");
    assert_eq!(
        read_source(&link).expect("read"),
        Source::Skipped(SkipReason::Symlink)
    );
}

#[test]
fn open_refuses_a_symlink_inside_the_root_too() {
    let root = TempDir::new("read");
    write(root.path(), "lib.rs", b"pub fn kept() {}\n");
    let link = root.path().join("link_in.rs");
    symlink(root.path().join("lib.rs"), &link).expect("link in");
    assert_eq!(
        read_source(&link).expect("read"),
        Source::Skipped(SkipReason::Symlink)
    );
}

#[test]
fn index_never_stores_an_outside_symlink_target() {
    let repo = FixtureRepo::new();
    let (_db_dir, store, report) = index_into_store(repo.root());
    assert!(store.match_count("open_ledger").expect("match") >= 1);
    assert_eq!(store.match_count("outside_root").expect("match"), 0);
    assert!(store.read_items("link_out.rs").expect("read").is_empty());
    assert!(!report.written.iter().any(|path| path == "link_out.rs"));
}

#[test]
fn open_skips_a_fifo_without_waiting_for_a_writer() {
    let root = TempDir::new("read");
    let fifo = root.path().join("pipe.rs");
    make_fifo(&fifo);
    assert_eq!(
        read_source(&fifo).expect("read"),
        Source::Skipped(SkipReason::NotRegular)
    );
}

#[test]
fn open_skips_a_device() {
    assert_eq!(
        read_source(Path::new("/dev/null")).expect("read"),
        Source::Skipped(SkipReason::NotRegular)
    );
}

#[test]
fn index_skips_a_fifo_named_like_rust() {
    let root = TempDir::new("repo");
    write(root.path(), "lib.rs", b"pub fn kept() {}\n");
    make_fifo(&root.path().join("pipe.rs"));
    let (_db_dir, store, report) = index_into_store(root.path());
    assert_eq!(report.written, ["lib.rs"]);
    let pipe = Skipped {
        path: "pipe.rs".to_owned(),
        reason: SkipReason::NotRegular,
    };
    assert!(report.skipped.contains(&pipe), "{:?}", report.skipped);
    assert!(store.read_items("pipe.rs").expect("read").is_empty());
    assert_eq!(store.row_count("files").expect("count"), 1);
}

#[test]
fn read_skips_a_file_one_byte_over_the_cap() {
    let root = TempDir::new("read");
    write(
        root.path(),
        "big.rs",
        &rust_bytes(BIG_HEAD, MAX_FILE_BYTES + 1),
    );
    assert_eq!(
        read_source(&root.path().join("big.rs")).expect("read"),
        Source::Skipped(SkipReason::TooLarge)
    );
}

#[test]
fn read_skips_a_file_far_over_the_cap() {
    let root = TempDir::new("read");
    write(
        root.path(),
        "huge.rs",
        &rust_bytes(BIG_HEAD, 4 * MAX_FILE_BYTES),
    );
    assert_eq!(
        read_source(&root.path().join("huge.rs")).expect("read"),
        Source::Skipped(SkipReason::TooLarge)
    );
}

#[test]
fn read_keeps_a_file_of_exactly_the_cap() {
    let root = TempDir::new("read");
    let edge = rust_bytes(EDGE_HEAD, MAX_FILE_BYTES);
    write(root.path(), "edge.rs", &edge);
    match read_source(&root.path().join("edge.rs")).expect("read") {
        Source::Bytes { bytes, mtime_ns } => {
            assert_eq!(bytes, edge);
            assert!(mtime_ns > 0, "{mtime_ns}");
        }
        Source::Skipped(reason) => panic!("skipped: {reason:?}"),
    }
}

#[test]
fn index_stores_the_cap_file_and_no_prefix_of_a_longer_one() {
    let repo = FixtureRepo::new();
    let (_db_dir, store, report) = index_into_store(repo.root());
    assert!(report.written.iter().any(|path| path == "edge.rs"));
    let edge_symbol = Item {
        kind: ItemKind::Function,
        name: "edge_symbol".to_owned(),
        visibility: Visibility::Public,
        start_line: 1,
        end_line: 0,
        signature: String::new(),
        doc_summary: None,
        is_test: false,
    };
    let items = without_detail(store.read_items("edge.rs").expect("read"));
    assert!(items.contains(&edge_symbol), "{items:?}");
    assert!(store.match_count("edge_symbol").expect("match") >= 1);
    assert!(store.read_items("big.rs").expect("read").is_empty());
    assert_eq!(store.match_count("big_prefix").expect("match"), 0);
}

#[test]
fn open_skips_both_names_of_a_hard_linked_outside_file() {
    let root = TempDir::new("read");
    let outside = TempDir::new("outside");
    write(outside.path(), "hard_target.rs", HARD_OUT_TEXT);
    let target = outside.path().join("hard_target.rs");
    let inside = root.path().join("hard_out.rs");
    fs::hard_link(&target, &inside).expect("hard link out");
    for path in [&inside, &target] {
        assert_eq!(
            read_source(path).expect("read"),
            Source::Skipped(SkipReason::HardLink),
            "{}",
            path.display()
        );
    }
}

#[test]
fn open_reads_a_file_again_once_its_other_link_is_gone() {
    let root = TempDir::new("read");
    write(root.path(), "lib.rs", b"pub fn kept() {}\n");
    let path = root.path().join("lib.rs");
    let other = root.path().join("lib_copy.rs");
    fs::hard_link(&path, &other).expect("hard link in");
    assert_eq!(
        read_source(&path).expect("read"),
        Source::Skipped(SkipReason::HardLink)
    );
    fs::remove_file(&other).expect("unlink");
    assert!(matches!(
        read_source(&path).expect("read"),
        Source::Bytes { .. }
    ));
}

#[test]
fn index_never_stores_a_hard_linked_outside_file() {
    let repo = FixtureRepo::new();
    let (_db_dir, store, report) = index_into_store(repo.root());
    let hard = Skipped {
        path: "hard_out.rs".to_owned(),
        reason: SkipReason::HardLink,
    };
    assert!(report.skipped.contains(&hard), "{:?}", report.skipped);
    assert!(store.match_count("open_ledger").expect("match") >= 1);
    assert_eq!(store.match_count("hard_outside").expect("match"), 0);
    assert!(store.read_items("hard_out.rs").expect("read").is_empty());
}
