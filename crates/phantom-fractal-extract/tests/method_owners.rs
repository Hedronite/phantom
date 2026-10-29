//! The `(method, type)` owner pairs an index pass hands the store
//! (SPEC-w6 G0b): the `self-method` owner match, never a name match.

mod common;

use common::{TempDir, write};
use phantom_fractal_extract::{index, stored_method_owners};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

/// A lib with two `Thing` types in two files, and a bin that implements
/// its own trait for one of them.
const FILES: [(&str, &str); 7] = [
    ("Cargo.toml", "[workspace]\nmembers = [\"kit\", \"app\"]\n"),
    ("kit/Cargo.toml", "[package]\nname = \"kit\"\n"),
    (
        "kit/src/lib.rs",
        "pub mod a;\npub mod b;\n\npub trait Show {\n    fn show(&self);\n}\n\n\
         pub enum Mode {\n    On,\n    Off,\n}\n\n\
         impl Mode {\n    pub fn flip(&self) -> Self {\n        Mode::Off\n    }\n}\n\n\
         impl Missing {\n    fn gone() {}\n}\n\n\
         impl dyn Show {\n    fn dynamic(&self) {}\n}\n\n\
         impl Show {\n    fn bare(&self) {}\n}\n",
    ),
    (
        "kit/src/a.rs",
        "pub struct Thing;\n\nimpl Thing {\n    pub fn alpha(&self) {}\n}\n",
    ),
    (
        "kit/src/b.rs",
        "pub struct Thing;\n\nimpl Thing {\n    pub fn beta(&self) {}\n}\n\n\
         impl crate::Show for Thing {\n    fn show(&self) {}\n}\n",
    ),
    (
        "app/Cargo.toml",
        "[package]\nname = \"app\"\n\n[dependencies]\nkit = { path = \"../kit\" }\n",
    ),
    (
        "app/src/main.rs",
        "trait Speak {\n    fn speak(&self);\n}\n\n\
         impl Speak for kit::a::Thing {\n    fn speak(&self) {}\n}\n\n\
         struct Local;\n\nimpl Local {\n    fn run(&self) {}\n}\n\nfn main() {}\n",
    ),
];

/// Every owner pair as `file method -> file type`, sorted.
fn owners() -> Vec<String> {
    let root = TempDir::new("owners");
    for (path, text) in FILES {
        write(root.path(), path, text.as_bytes());
    }
    let db = TempDir::new("owners-db");
    let mut store = Store::open(&db.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    let report = index(root.path(), &mut store, &rust).expect("index");
    assert!(report.graph.is_some(), "the first pass rebuilds");
    let input = store.resolve_input().expect("input");
    let label = |id| {
        let symbol = input
            .symbols
            .iter()
            .find(|symbol| symbol.id == id)
            .expect("symbol");
        format!("{} {}", symbol.file, symbol.name)
    };
    let mut out: Vec<String> = stored_method_owners(&store, root.path())
        .expect("owners")
        .into_iter()
        .map(|(method, ty)| format!("{} -> {}", label(method), label(ty)))
        .collect();
    out.sort();
    out
}

#[test]
fn two_things_in_two_files_each_own_only_their_own_methods() {
    let got = owners();
    let things: Vec<&str> = got
        .iter()
        .map(String::as_str)
        .filter(|pair| pair.ends_with(" Thing"))
        .collect();
    assert_eq!(
        things,
        [
            "kit/src/a.rs alpha -> kit/src/a.rs Thing",
            "kit/src/b.rs beta -> kit/src/b.rs Thing",
            "kit/src/b.rs show -> kit/src/b.rs Thing",
        ]
    );
}

#[test]
fn only_methods_owned_by_one_type_in_their_own_target_get_a_pair() {
    // `gone` names no type, `dynamic` sits on a trait object, `bare`'s self
    // type resolves to the trait `Show`, and `speak` is a bin's impl on the
    // lib's type: none gets a pair.
    assert_eq!(
        owners(),
        [
            "app/src/main.rs run -> app/src/main.rs Local",
            "kit/src/a.rs alpha -> kit/src/a.rs Thing",
            "kit/src/b.rs beta -> kit/src/b.rs Thing",
            "kit/src/b.rs show -> kit/src/b.rs Thing",
            "kit/src/lib.rs flip -> kit/src/lib.rs Mode",
        ]
    );
}
