//! The receiver hints and the signatures the G2b store wire
//! carries: put from the extractor, read back through the resolver,
//! dumped, never rewritten by a second resolve.

use super::{TempDb, assert_outside_workspace};
use crate::Store;
use phantom_fractal_lang_rust::RustLanguage;

fn language() -> RustLanguage {
    RustLanguage::new().expect("grammar")
}

/// The typed fixture's source (SPEC-w10 G2b): one ctor hint on the
/// method call, plus a plain path call beside it.
const TYPED_SOURCE: &str = "pub struct Counter;\n\
    impl Counter {\n\
        pub fn new() -> Counter {\n\
            Counter\n\
        }\n\
        pub fn check(&self) -> u8 {\n\
            1\n\
        }\n\
    }\n\
    pub fn entry() -> u8 {\n\
        let counter = Counter::new();\n\
        counter.check()\n\
    }\n";

/// The hint the extractor writes for the method call, read back from
/// the resolver input, with every symbol's signature filled from
/// `symbols.signature`.
#[test]
fn a_receiver_hint_and_the_signatures_round_trip() {
    use phantom_fractal_lang::RefKind;

    let db = TempDb::new("rttyped");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    store
        .put_source("src/round.rs", TYPED_SOURCE, 0, &language)
        .expect("put typed source");
    let input = store.resolve_input().expect("resolve");
    let hinted: Vec<&phantom_fractal_lang::CallRef> = input
        .refs
        .iter()
        .filter(|reference| reference.receiver_type.is_some())
        .collect();
    assert_eq!(hinted.len(), 1, "{:?}", input.refs.len());
    assert_eq!(hinted[0].kind, RefKind::MethodCall);
    assert_eq!(
        hinted[0].receiver_type.as_deref(),
        Some("ctor:Counter::new")
    );
    // Every symbol carries its signature from the stored row.
    for symbol in &input.symbols {
        assert!(
            symbol
                .signature
                .as_deref()
                .is_some_and(|text| !text.is_empty()),
            "{}",
            symbol.name
        );
    }
    let entry = input
        .symbols
        .iter()
        .find(|symbol| symbol.name == "entry")
        .expect("entry");
    assert!(
        entry
            .signature
            .as_deref()
            .is_some_and(|text| text.contains("-> u8"))
    );
    drop(store);
    drop(language);
}

/// The null case: a path call with no hint stores none, the resolver
/// reads none, and the dump's own column reads `\N`.
#[test]
fn a_hintless_call_stays_null_in_the_row_and_the_dump() {
    use phantom_fractal_lang::RefKind;

    let db = TempDb::new("nulltyped");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    store
        .put_source(
            "src/null.rs",
            "pub fn kicker() -> u8 {\n    kicker()\n}\n",
            0,
            &language,
        )
        .expect("put");
    let input = store.resolve_input().expect("resolve");
    let call = input
        .refs
        .iter()
        .find(|reference| reference.kind == RefKind::Call)
        .expect("the call");
    assert_eq!(call.receiver_type, None);
    assert!(
        store
            .canonical_dump()
            .expect("dump")
            .lines()
            .any(|line| line.contains("\\N") && line.contains("kicker")),
    );
    drop(store);
    drop(language);
}

/// The dump changes when a hint changes: the same fn, once with the
/// ctor binding and once with a clear, dumps differently.
#[test]
fn the_dump_changes_when_a_hint_changes() {
    let db = TempDb::new("dumped");
    assert_outside_workspace(db.path());
    let language = language();
    let mut store = Store::open(db.path()).expect("open");
    store
        .put_source("src/hint.rs", TYPED_SOURCE, 0, &language)
        .expect("hintful put");
    let hinted = store.canonical_dump().expect("dump");
    // A hintless twin: the `counter.check()` ref keeps its shape but
    // loses the hint, so only the hint cell moves.
    store
        .put_source(
            "src/hint.rs",
            "pub struct Counter;\n\
             impl Counter {\n\
                 pub fn new() -> Counter {\n\
                     Counter\n\
                 }\n\
                 pub fn check(&self) -> u8 {\n\
                     1\n\
                 }\n\
             }\n\
             pub fn entry() -> u8 {\n\
                 let counter;\n\
                 counter = Counter::new();\n\
                 counter.check()\n\
             }\n",
            1,
            &language,
        )
        .expect("hintless put");
    let cleared = store.canonical_dump().expect("dump");
    assert_ne!(hinted, cleared, "the hint left the dump equal");
    assert_eq!(cleared, store.canonical_dump().expect("stable"));
    drop(store);
    drop(language);
}

/// A second resolve on the unchanged store: the hints and signatures
/// come back equal, and the data dir never moves.
#[test]
fn the_second_resolve_pass_writes_nothing() {
    let db = TempDb::new("pstatic");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    store
        .put_source("src/entry.rs", TYPED_SOURCE, 0, &language)
        .expect("put");
    drop(language);
    let store_bytes = |path: &std::path::Path| {
        let bytes_files = u128::from(std::fs::metadata(path).expect("db").len());
        let text = path.to_string_lossy().into_owned();
        let wal = u128::from(
            std::fs::metadata(format!("{text}-wal"))
                .map(|meta| meta.len())
                .unwrap_or(0),
        );
        (bytes_files, wal)
    };
    let first = store.resolve_input().expect("first resolve");
    let (db_bytes, wal_bytes) = store_bytes(db.path());
    let second = store.resolve_input().expect("second resolve");
    assert_eq!(first.refs, second.refs, "the hints moved");
    assert_eq!(first.symbols, second.symbols, "the signatures moved");
    assert_eq!(store_bytes(db.path()), (db_bytes, wal_bytes));
    drop(store);
}
