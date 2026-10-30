//! The `root` and `index_head` meta setters (W9 D4): a set writes only
//! on change, and the `index_head` of a git-less root stores as the
//! empty string and reads back as `None`.

use std::fs;
use std::path::Path;

use super::{TempDb, assert_outside_workspace};
use crate::Store;

/// The size of the database and its WAL, in bytes, to catch a no-op
/// set that still touches the store.
fn store_bytes(path: &Path) -> (u128, u128) {
    let file = |suffix: &str| {
        let text = path.to_string_lossy().into_owned();
        fs::metadata(format!("{text}{suffix}"))
            .map(|meta| meta.len())
            .unwrap_or(0) as u128
    };
    (file(""), file("-wal"))
}

#[test]
fn setters_write_only_on_change() {
    let db = TempDb::new("setters");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    assert_eq!(store.root().expect("unset root"), None);
    assert_eq!(store.index_head().expect("unset head"), None);

    // The first write lands and reads back.
    assert!(store.set_root("/roots/ledger").expect("set root"));
    assert_eq!(
        store.root().expect("root"),
        Some("/roots/ledger".to_owned())
    );
    // The same value writes nothing.
    let (before, wal) = store_bytes(db.path());
    assert!(!store.set_root("/roots/ledger").expect("re-set root"));
    assert_eq!(store_bytes(db.path()), (before, wal));
    // A differing value writes once.
    assert!(store.set_root("/roots/very-ledger").expect("set root"));
    assert_eq!(
        store.root().expect("root"),
        Some("/roots/very-ledger".to_owned())
    );

    // A git head writes, repeats as nothing, and a git-less root — the
    // stored empty string — clears it.
    assert!(store.set_index_head(Some("abcddcba")).expect("set head"));
    assert_eq!(
        store.index_head().expect("head"),
        Some("abcddcba".to_owned())
    );
    assert!(!store.set_index_head(Some("abcddcba")).expect("re-set"));
    let (before, wal) = store_bytes(db.path());
    assert!(!store.set_index_head(Some("abcddcba")).expect("re-set"));
    assert_eq!(store_bytes(db.path()), (before, wal));
    assert!(store.set_index_head(None).expect("clear head"));
    assert_eq!(store.index_head().expect("cleared head"), None);
    assert!(!store.set_index_head(None).expect("re-clear"));
    let (before, wal) = store_bytes(db.path());
    assert!(!store.set_index_head(None).expect("re-clear"));
    assert_eq!(store_bytes(db.path()), (before, wal));
    // And a fresh head writes again over the cleared value.
    assert!(
        store
            .set_index_head(Some("0123456789abcdef"))
            .expect("set head")
    );
    assert_eq!(
        store.index_head().expect("head"),
        Some("0123456789abcdef".to_owned())
    );
}

#[test]
fn an_older_head_is_not_confused_with_the_empty_string() {
    let db = TempDb::new("headvalues");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // A one-character head is a value, not the empty representation of
    // `None`; switching between the two always writes.
    assert!(store.set_index_head(Some("!")).expect("set"));
    assert_eq!(store.index_head().expect("one char"), Some("!".to_owned()));
    assert!(store.set_index_head(None).expect("clear"));
    assert_eq!(store.index_head().expect("none"), None);
}
