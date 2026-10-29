//! Store detail reads (SPEC-w6 G0b): the extractor's three columns round
//! trip, `symbol_detail` reads every `fixtures/detail` row back equal,
//! stable-key lookup works, and `members` matches the W4 owner rule.

use std::fs;
use std::path::{Path, PathBuf};

use phantom_fractal_lang::{Language, Visibility};
use phantom_fractal_lang_rust::RustLanguage;

use crate::{Store, SymbolDetail};

/// The fixture's `src/*.rs` files, sorted, so walk order matches
/// `fixtures/detail/EXPECTED.md`.
fn file_paths() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/detail/src");
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&root)
        .expect("fixture dir")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .collect();
    entries.sort();
    for path in entries {
        let relative = path.strip_prefix(&root).unwrap();
        let rel = relative.to_string_lossy().replace('\\', "/");
        out.push((path, rel));
    }
    out
}

#[test]
fn fixtures_detail_rows_read_back_through_symbol_detail() {
    let db = super::TempDb::new("detail");
    super::assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");

    let mut checked = 0;
    for (absolute, relative) in file_paths() {
        let source = fs::read_to_string(&absolute).expect("read fixture");
        let extract = language.extract(&source).expect("extract");
        store
            .put_source(&relative, &source, 0, &language)
            .expect("put");

        for (position, item) in extract.items.iter().enumerate() {
            let ordinal = extract.items[..position]
                .iter()
                .filter(|prior| prior.name == item.name && prior.kind == item.kind)
                .count();
            let stable_key = format!("{relative}#{}#{}#{ordinal}", item.name, item.kind.as_str());
            let scope = &extract.item_scopes[position];
            let qual_name = if scope.is_empty() {
                item.name.clone()
            } else {
                format!("{scope}::{}", item.name)
            };
            let id = store
                .symbol_by_stable_key(&stable_key)
                .expect("key query")
                .unwrap_or_else(|| panic!("no row for {stable_key}"));
            let detail = store.symbol_detail(id).expect("detail").expect("row");
            let visibility_text = match &item.visibility {
                Visibility::Private => "private".to_owned(),
                Visibility::Public => "public".to_owned(),
                Visibility::Crate => "crate".to_owned(),
                Visibility::Restricted(text) => format!("restricted:{text}"),
            };
            assert_row(
                &detail,
                &stable_key,
                item.kind.as_str(),
                item.name.as_str(),
                &qual_name,
                &item.signature,
                item.doc_summary.as_deref(),
                item.start_line,
                item.end_line,
                item.is_test,
                &visibility_text,
            );
            checked += 1;
        }
        for (position, method) in extract.methods.iter().enumerate() {
            let ordinal = extract.methods[..position]
                .iter()
                .filter(|prior| prior.name == method.name)
                .count();
            let stable_key = format!(
                "{relative}#{}::{}#method#{ordinal}",
                method.owner, method.name
            );
            let scope = &method.scope;
            let qual_name = if scope.is_empty() {
                format!("{}::{}", method.owner, method.name)
            } else {
                format!("{scope}::{}::{}", method.owner, method.name)
            };
            let id = store
                .symbol_by_stable_key(&stable_key)
                .expect("key query")
                .unwrap_or_else(|| panic!("no row for {stable_key}"));
            let detail = store.symbol_detail(id).expect("detail").expect("row");
            let visibility_text = match &method.visibility {
                Visibility::Private => "private".to_owned(),
                Visibility::Public => "public".to_owned(),
                Visibility::Crate => "crate".to_owned(),
                Visibility::Restricted(text) => format!("restricted:{text}"),
            };
            assert_row(
                &detail,
                &stable_key,
                "method",
                method.name.as_str(),
                &qual_name,
                &method.signature,
                method.doc_summary.as_deref(),
                method.start_line,
                method.end_line,
                method.is_test,
                &visibility_text,
            );
            checked += 1;
        }
    }
    // At least as many rows as `fixtures/detail/EXPECTED.md` lists.
    assert!(checked >= 20, "checked {checked}");
    drop(store);
}

/// One row's stored values, compared against the extract's own fields.
#[allow(clippy::too_many_arguments)]
fn assert_row(
    detail: &SymbolDetail,
    stable_key: &str,
    kind: &str,
    name: &str,
    qual_name: &str,
    signature: &str,
    doc_summary: Option<&str>,
    start_line: u32,
    end_line: u32,
    is_test: bool,
    visibility: &str,
) {
    assert_eq!(detail.stable_key, stable_key, "stable_key of {stable_key}");
    assert_eq!(detail.kind, kind, "{stable_key}");
    assert_eq!(detail.name, name, "{stable_key}");
    assert_eq!(detail.qual_name, qual_name, "{stable_key}");
    assert_eq!(detail.signature.as_deref(), Some(signature), "{stable_key}");
    assert_eq!(detail.doc_summary.as_deref(), doc_summary, "{stable_key}");
    assert_eq!(
        detail.start_line,
        Some(i64::from(start_line)),
        "{stable_key}"
    );
    assert_eq!(detail.end_line, Some(i64::from(end_line)), "{stable_key}");
    assert_eq!(detail.is_test, is_test, "{stable_key}");
    assert_eq!(detail.visibility, visibility, "{stable_key} visibility");
}

#[test]
fn members_match_the_w4_owner_rule() {
    let db = super::TempDb::new("members");
    super::assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    let source = "pub struct Ledger {\n    entries: u32,\n}\n\nimpl Ledger {\n    pub fn open(&self) -> u32 {\n        0\n    }\n\n    pub fn drain(&self) {}\n}\n\nimpl Measure for Ledger {\n    fn measure(&self) -> u32 {\n        1\n    }\n}\n\npub enum Shape {\n    Point,\n}\n\npub fn freefn() {}\n";
    store
        .put_source("src/ledger.rs", source, 0, &language)
        .expect("put");
    let ledger = store
        .symbol_by_stable_key("src/ledger.rs#Ledger#struct#0")
        .expect("key query")
        .expect("ledger row");
    // The resolver pairs each ledger method with the type; without the
    // write, members read back empty under the parent_id rule.
    let method_open = store
        .symbol_by_stable_key("src/ledger.rs#Ledger::open#method#0")
        .expect("key query")
        .expect("method open");
    let method_drain = store
        .symbol_by_stable_key("src/ledger.rs#Ledger::drain#method#0")
        .expect("key query")
        .expect("method drain");
    let method_measure = store
        .symbol_by_stable_key("src/ledger.rs#Ledger::measure#method#0")
        .expect("key query")
        .expect("method measure");
    store
        .write_method_owners(&[
            (method_open, ledger),
            (method_drain, ledger),
            (method_measure, ledger),
        ])
        .expect("owners");
    let members = store.members(ledger).expect("members");
    assert_eq!(members.len(), 3, "inherent and trait impls both count");
    assert_eq!(members[0].name, "open");
    assert_eq!(members[1].name, "drain");
    assert_eq!(members[2].name, "measure");
    assert!(
        members
            .iter()
            .all(|member| member.stable_key.starts_with("src/ledger.rs#"))
    );
    // Ordered by line: open, drain, measure.
    assert!(
        members[0].start_line <= members[1].start_line
            && members[1].start_line <= members[2].start_line
    );
    // An enum's members are its impl methods; a free fn has none.
    let shape = store
        .symbol_by_stable_key("src/ledger.rs#Shape#enum#0")
        .expect("key query")
        .expect("shape row");
    assert!(store.members(shape).expect("members").is_empty());
    drop(store);
}

#[test]
fn members_is_empty_for_non_member_kinds() {
    let db = super::TempDb::new("members-empty");
    super::assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    let source =
        "pub fn freefn() {}\npub struct Ledger;\nimpl Ledger {\n    pub fn open(&self) {}\n}\n";
    store
        .put_source("src/plain.rs", source, 0, &language)
        .expect("put");
    let free_id = store
        .symbol_by_stable_key("src/plain.rs#freefn#function#0")
        .expect("key query")
        .expect("free fn row");
    // A fn is not a struct, enum or trait, so members stay empty even
    // though its name collides with another type's method names would
    // have matched.
    assert!(store.members(free_id).expect("members").is_empty());
    let detail = store.symbol_detail(free_id).expect("detail").expect("row");
    assert_eq!(detail.path, "src/plain.rs");
    drop(store);
}

#[test]
fn write_method_owners_resolves_two_same_named_types() {
    let db = super::TempDb::new("thingtwice");
    super::assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    let source = "pub struct Thing;\n\nimpl Thing {\n    pub fn a(&self) {}\n}\n";
    store
        .put_source("src/a.rs", source, 0, &language)
        .expect("put a");
    let other = "pub struct Thing;\n\nimpl Thing {\n    pub fn b(&self) {}\n}\n";
    store
        .put_source("src/b.rs", other, 0, &language)
        .expect("put b");
    // Both methods carry owner `Thing`: the name-only match from the
    // earlier G0b round would have paired both methods with both types.
    let thing_a = store
        .symbol_by_stable_key("src/a.rs#Thing#struct#0")
        .expect("key query")
        .expect("a.rs Thing");
    let thing_b = store
        .symbol_by_stable_key("src/b.rs#Thing#struct#0")
        .expect("key query")
        .expect("b.rs Thing");
    let method_a = store
        .symbol_by_stable_key("src/a.rs#Thing::a#method#0")
        .expect("key query")
        .expect("method a");
    let method_b = store
        .symbol_by_stable_key("src/b.rs#Thing::b#method#0")
        .expect("key query")
        .expect("method b");
    // The resolver pairs each method with its own type.
    store
        .write_method_owners(&[(method_a, thing_a), (method_b, thing_b)])
        .expect("owners");
    let members_a = store.members(thing_a).expect("members a");
    // A pass with no resolved pairs clears the previous attribution, so
    // a rebuild is exact.
    store.write_method_owners(&[]).expect("clearing pass");
    assert!(store.members(thing_a).expect("clear a").is_empty());
    assert!(store.members(thing_b).expect("clear b").is_empty());
    // The resolution pairs land again at the next rebuild.
    store
        .write_method_owners(&[(method_a, thing_a), (method_b, thing_b)])
        .expect("owners again");
    assert_eq!(members_a.len(), 1, "{members_a:?}");
    assert_eq!(members_a[0].id, method_a);
    assert_eq!(members_a[0].name, "a");
    let members_b = store.members(thing_b).expect("members b");
    assert_eq!(members_b.len(), 1, "{members_b:?}");
    assert_eq!(members_b[0].id, method_b);
    assert_eq!(members_b[0].name, "b");
    // Empty for a memberless kind and an unknown id.
    let missing = store.members(9999).expect("members");
    assert!(missing.is_empty());
    drop(store);
}

#[test]
fn symbol_detail_is_none_for_unknown_ids_and_keys() {
    let db = super::TempDb::new("detail-none");
    super::assert_outside_workspace(db.path());
    let store = Store::open(db.path()).expect("open");
    assert_eq!(store.symbol_detail(9999).expect("none"), None);
    assert_eq!(
        store
            .symbol_by_stable_key("missing.rs#x#function#0")
            .expect("none"),
        None
    );
}
