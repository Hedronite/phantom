//! Co-change history tests (SPEC-w8 G1b): the write replaces the scan
//! in one transaction, the reads answer bound, and an identical scan
//! leaves the db bytes unchanged.

use phantom_fractal_lang_rust::RustLanguage;

use super::{TempDb, assert_outside_workspace};
use crate::{CochangeScan, FileRow, PairRow, Store};

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

#[test]
fn cochange_round_trips_and_reads_empty() {
    let db = TempDb::new("cochange");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    // No scan landed: all reads answer nothing or empty.
    assert_eq!(store.cochange_scan().expect("unset"), None);
    // An empty (non-git) scan stores the head row but no pairs or files.
    store
        .write_cochange(&CochangeScan::default())
        .expect("empty scan");
    let scan = store.cochange_scan().expect("empty scan").expect("stored");
    assert_eq!(scan.last_oid, None);
    assert!(scan.files.is_empty() && scan.pairs.is_empty());
    drop(store);
}
#[test]
fn cochange_round_trips_full_scans_and_stays_writeless() {
    let db = TempDb::new("cochangefull");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct Ledger {\\n\\n}\\npub struct Other;\\n";
    store
        .put_source("src/a.rs", source, 0, &language)
        .expect("put a");
    let other = "pub fn pick() -> u8 {\\n    1\\n}\\n";
    store
        .put_source("src/b.rs", other, 0, &language)
        .expect("put b");
    let third = "pub struct Cap;\n";
    store
        .put_source("src/c.rs", third, 0, &language)
        .expect("put c");
    let scan = CochangeScan {
        last_oid: Some("aaaa1111".to_owned()),
        oldest_oid: Some("bbbb2222".to_owned()),
        commits_seen: 45,
        skipped_bulk: 3,
        window_json: "{\"days\":365,\"max_commits\":300}".to_owned(),
        files: vec![
            FileRow {
                path: "src/a.rs".to_owned(),
                commits: 2.0,
                last_time: 1_700_000_000,
                renamed_from: None,
            },
            FileRow {
                path: "src/b.rs".to_owned(),
                commits: 3.5,
                last_time: 1_700_001_000,
                renamed_from: Some("src/old.rs".to_owned()),
            },
            FileRow {
                path: "src/c.rs".to_owned(),
                commits: 4.0,
                last_time: 1_700_002_000,
                renamed_from: None,
            },
        ],
        pairs: vec![
            PairRow {
                a: "src/a.rs".to_owned(),
                b: "src/b.rs".to_owned(),
                together: 2.0,
                n_a: 2.0,
                n_b: 3.5,
                lift: 1.4,
                conf_ab: 0.5,
                conf_ba: 0.57,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_001_000,
            },
            PairRow {
                a: "src/a.rs".to_owned(),
                b: "src/c.rs".to_owned(),
                together: 4.0,
                n_a: 2.0,
                n_b: 4.0,
                lift: 2.5,
                conf_ab: 0.5,
                conf_ba: 0.9,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_002_000,
            },
            PairRow {
                a: "src/b.rs".to_owned(),
                b: "src/c.rs".to_owned(),
                together: 1.0,
                n_a: 3.5,
                n_b: 4.0,
                lift: 0.9,
                conf_ab: 0.3,
                conf_ba: 0.25,
                last_oid: "aaaa1111".to_owned(),
                last_time: 1_700_001_500,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    // The scan reads back both ends and the stored types.
    let stored = store.cochange_scan().expect("scan").expect("stored");
    assert_eq!(stored.last_oid.as_deref(), Some("aaaa1111"));
    assert_eq!(stored.commits_seen, 45);
    assert_eq!(stored.files.len(), 3);
    // Pairs store once as a < b by file id: a.rs(1) before b.rs(2)
    // before c.rs(3), with each pair recorded once with lower first.
    assert_eq!(stored.files[0].path, "src/a.rs");
    assert_eq!(stored.files[2].path, "src/c.rs");
    let pairs: Vec<Vec<String>> = stored
        .pairs
        .iter()
        .map(|pair| vec![pair.a.clone(), pair.b.clone()])
        .collect();
    assert_eq!(
        pairs,
        vec![
            vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()],
            vec!["src/a.rs".to_owned(), "src/c.rs".to_owned()],
            vec!["src/b.rs".to_owned(), "src/c.rs".to_owned()],
        ],
        "{pairs:?}"
    );
    assert!(stored.pairs.iter().all(|pair| pair.together > 0.0));
    // Partners read per path, ordered by lift desc.
    // Partners read per path, ordered by lift desc: c.rs (2.5) first,
    // b.rs (1.4) second.
    let partners = store.cochange_of("src/a.rs", 1.0, 10).expect("partners");
    assert_eq!(partners.len(), 2, "{partners:?}");
    assert_eq!(partners[0].path.as_deref(), Some("src/c.rs"));
    assert_eq!(partners[0].lift, 2.5);
    assert_eq!(partners[1].path.as_deref(), Some("src/b.rs"));
    assert_eq!(partners[1].lift, 1.4);
    // A lift filter drops partners under the stored value.
    let filtered = store.cochange_of("src/a.rs", 1.5, 10).expect("filtered");
    assert_eq!(filtered.len(), 1, "{filtered:?}");
    assert_eq!(filtered[0].path.as_deref(), Some("src/c.rs"));
    // A lift filter above every stored value answers nothing.
    assert!(
        store
            .cochange_of("src/a.rs", 9.0, 10)
            .expect("filtered")
            .is_empty()
    );
    // The limit caps the partner list from the top.
    let capped = store.cochange_of("src/a.rs", 1.0, 1).expect("limit");
    assert_eq!(capped.len(), 1, "{capped:?}");
    assert_eq!(capped[0].path.as_deref(), Some("src/c.rs"));
    assert_eq!(
        store
            .cochange_of("src/a.rs", 1.0, 0)
            .expect("zero limit")
            .len(),
        0
    );
    // Identical scan writes leave the db bytes unchanged (db and wal).
    let db_path = db.path().to_string_lossy().into_owned();
    let before = std::fs::read(&db_path).expect("db read");
    store.write_cochange(&scan).expect("rewrite");
    let after_db = std::fs::read(&db_path).expect("db read");
    assert_eq!(before, after_db, "identical scan rewrites nothing");
    // cochange_head in meta carries the scan's head.
    assert_eq!(
        store.cochange_head().expect("head").as_deref(),
        Some("aaaa1111")
    );
    // Per-file partners: the map's read caps per file and dedups paths.
    let groups = store
        .cochange_partners_for_files(
            &[
                "src/a.rs".to_owned(),
                "src/a.rs".to_owned(),
                "src/b.rs".to_owned(),
            ],
            1.0,
            5,
        )
        .expect("groups");
    assert_eq!(groups.len(), 2, "identical paths answer once");
    assert_eq!(groups[0].path, "src/a.rs");
    drop(store);
    drop(language);
}
#[test]
fn cochange_of_unknown_path_and_drop_unresolved_pairs() {
    let db = TempDb::new("cochangemiss");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct One;\\n";
    store
        .put_source("src/one.rs", source, 0, &language)
        .expect("put");
    // A pair naming a path the index dropped cannot be stored: the end
    // resolves no files row, so the write drops that pair.
    let scan = CochangeScan {
        last_oid: Some("cccc3333".to_owned()),
        oldest_oid: Some("cccc3333".to_owned()),
        commits_seen: 2,
        skipped_bulk: 0,
        window_json: "{}".to_owned(),
        files: vec![FileRow {
            path: "src/one.rs".to_owned(),
            commits: 1.0,
            last_time: 10,
            renamed_from: None,
        }],
        pairs: vec![
            PairRow {
                a: "src/one.rs".to_owned(),
                b: "src/gone.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 2.0,
                conf_ab: 1.0,
                conf_ba: 1.0,
                last_oid: "cccc3333".to_owned(),
                last_time: 10,
            },
            PairRow {
                a: "src/one.rs".to_owned(),
                b: "src/one.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 2.0,
                conf_ab: 1.0,
                conf_ba: 1.0,
                last_oid: "cccc3333".to_owned(),
                last_time: 10,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    let stored = store.cochange_scan().expect("scan").expect("stored");
    assert_eq!(stored.pairs.len(), 0, "{stored:?}");
    drop(store);
    drop(language);
}
#[test]
fn cochange_partners_order_lift_then_together_then_path() {
    let db = TempDb::new("cochangeorder");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let source = "pub struct One;\\npub struct Two;\\npub struct Three;\\n";
    store
        .put_source("src/order.rs", source, 0, &language)
        .expect("put");
    let scan = CochangeScan {
        last_oid: Some("dddd4444".to_owned()),
        oldest_oid: Some("dddd4444".to_owned()),
        commits_seen: 10,
        skipped_bulk: 0,
        window_json: "{}".to_owned(),
        files: vec![FileRow {
            path: "src/order.rs".to_owned(),
            commits: 4.0,
            last_time: 5,
            renamed_from: None,
        }],
        pairs: vec![
            PairRow {
                a: "src/order.rs".to_owned(),
                b: "src/one.rs".to_owned(),
                together: 1.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 1.2,
                conf_ab: 0.5,
                conf_ba: 0.6,
                last_oid: "dddd4444".to_owned(),
                last_time: 5,
            },
            PairRow {
                a: "src/order.rs".to_owned(),
                b: "src/two.rs".to_owned(),
                together: 2.0,
                n_a: 1.0,
                n_b: 1.0,
                lift: 1.2,
                conf_ab: 0.5,
                conf_ba: 0.6,
                last_oid: "dddd4444".to_owned(),
                last_time: 4,
            },
        ],
    };
    store.write_cochange(&scan).expect("write");
    // These targets are outside the index: their ends resolved no files
    // rows, so no partner rows carry them; keep them out.
    let partners = store
        .cochange_of("src/order.rs", 1.0, 10)
        .expect("partners");
    assert!(partners.is_empty(), "{partners:?}");
    drop(store);
    drop(language);
}
