//! Re-indexing after an edit (W9 G0 fix). A second pass over an edited
//! tree must succeed and leave a `canonical_dump` equal to a fresh index
//! of the edited tree, which is what `fract watch` relies on (D6).
//!
//! Two ab4d726 failures this covers:
//! - a method whose `impl` names a type in another file (`impl
//!   super::Counter` in `resolve-self/src/shapes.rs`) takes that type's
//!   row as its `parent_id`, so replacing or removing the type's file
//!   failed the pass with "FOREIGN KEY constraint failed";
//! - a symbol marked a test because its module was a test module kept
//!   the mark after that module stopped being one (`impact` without
//!   `engine/src/lib.rs`, whose `#[cfg(test)] mod tests;` made
//!   `engine/src/tests.rs` a test module).

mod common;
mod fixture;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TempDir, copy_tree};
use phantom_fractal_extract::{index, walk};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

/// A tree copied from a fixture, and a store holding its index.
struct Indexed {
    repo: TempDir,
    _db_dir: TempDir,
    store: Store,
    rust: RustLanguage,
}

impl Indexed {
    fn new(fixture: &Path) -> Self {
        let repo = TempDir::new("reindex");
        copy_tree(fixture, repo.path());
        let db_dir = TempDir::new("reindex-db");
        let store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
        let rust = RustLanguage::new().expect("query compiles");
        let mut out = Self {
            repo,
            _db_dir: db_dir,
            store,
            rust,
        };
        out.index().expect("first pass");
        out
    }

    fn index(&mut self) -> Result<(), String> {
        index(self.repo.path(), &mut self.store, &self.rust)
            .map(drop)
            .map_err(|err| format!("{err:?}"))
    }

    /// The second pass succeeds, and its dump is a fresh index's.
    fn assert_reindexes(&mut self, what: &str) {
        if let Err(err) = self.index() {
            panic!("{what}: second pass: {err}");
        }
        let got = self.store.canonical_dump().expect("dump");
        let want = fresh_dump(self.repo.path());
        let first = got
            .lines()
            .zip(want.lines())
            .find(|(got, want)| got != want);
        assert!(
            got == want,
            "{what}: the re-indexed dump is not a fresh index's; first difference {first:?}"
        );
    }
}

/// A fresh index of `root` into a new store, as a canonical dump.
fn fresh_dump(root: &Path) -> String {
    let db_dir = TempDir::new("reindex-fresh-db");
    let mut store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    index(root, &mut store, &rust).expect("fresh index");
    store.canonical_dump().expect("dump")
}

/// Every fixture directory with a `Cargo.toml` at its top.
fn fixtures() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(FIXTURES)
        .expect("fixtures")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.join("Cargo.toml").is_file())
        .collect();
    out.sort();
    out
}

/// The `.rs` paths an index of `fixture` keeps.
fn sources(fixture: &Path) -> Vec<String> {
    walk(fixture)
        .expect("walk")
        .files
        .into_iter()
        .map(|file| file.path)
        .filter(|path| path.ends_with(".rs"))
        .collect()
}

fn name(fixture: &Path) -> String {
    fixture
        .file_name()
        .expect("named")
        .to_string_lossy()
        .into_owned()
}

fn append(root: &Path, rel: &str) {
    let path = root.join(rel);
    let mut text = fs::read(&path).expect("read");
    text.extend_from_slice(b"// again\n");
    fs::write(&path, text).expect("append");
}

#[test]
fn an_appended_comment_reindexes_resolve_self() {
    let mut s = Indexed::new(&Path::new(FIXTURES).join("resolve-self"));
    append(s.repo.path(), "src/lib.rs");
    s.assert_reindexes("resolve-self src/lib.rs");
}

#[test]
fn a_test_module_that_loses_its_declaration_is_no_longer_a_test() {
    let mut s = Indexed::new(&Path::new(FIXTURES).join("impact"));
    fs::remove_file(s.repo.path().join("engine/src/lib.rs")).expect("delete");
    s.assert_reindexes("impact without engine/src/lib.rs");
}

#[test]
fn removing_the_file_of_a_type_another_file_implements_reindexes() {
    let mut s = Indexed::new(&Path::new(FIXTURES).join("resolve-self"));
    fs::remove_file(s.repo.path().join("src/lib.rs")).expect("delete");
    s.assert_reindexes("resolve-self without src/lib.rs");
}

#[test]
fn every_fixture_reindexes_after_an_append_to_each_file() {
    let mut checked = 0;
    for fixture in fixtures() {
        for rel in sources(&fixture) {
            let mut s = Indexed::new(&fixture);
            append(s.repo.path(), &rel);
            s.assert_reindexes(&format!("{} {rel}", name(&fixture)));
            checked += 1;
        }
    }
    assert!(checked >= 40, "{checked}");
}

#[test]
fn every_fixture_reindexes_after_each_file_is_removed() {
    for fixture in fixtures() {
        for rel in sources(&fixture) {
            let mut s = Indexed::new(&fixture);
            fs::remove_file(s.repo.path().join(&rel)).expect("delete");
            s.assert_reindexes(&format!("{} without {rel}", name(&fixture)));
        }
    }
}
