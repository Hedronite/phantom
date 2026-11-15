//! The W14 find reads: `symbols_named` over items and methods, the
//! module method spelling as a fallback, `did_you_mean`, and the
//! shared-name regression.
use super::graph::{language, put};
use super::{TempDb, assert_outside_workspace};
use crate::{Package, Store};

fn write_modules(store: &mut Store, modules: &[(&str, &str)]) {
    let modules: Vec<phantom_fractal_lang::Module> = modules
        .iter()
        .enumerate()
        .map(|(position, (path, file))| phantom_fractal_lang::Module {
            id: 100 + position as i64,
            target: 1,
            parent: None,
            path: (*path).to_owned(),
            file: (*file).to_owned(),
            scope: String::new(),
        })
        .collect();
    store
        .write_crate_graph(
            &[Package {
                name: "sample".to_owned(),
                version: None,
                manifest_path: "Cargo.toml".to_owned(),
            }],
            &[phantom_fractal_lang::Target {
                id: 1,
                package: "sample".to_owned(),
                name: "sample".to_owned(),
                kind: phantom_fractal_lang::TargetKind::Lib,
                root: "src/lib.rs".to_owned(),
            }],
            &modules,
            &[],
        )
        .expect("graph");
}

#[test]
fn find_rows_carry_methods_as_well_as_items() {
    let db = TempDb::new("findrows");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let app = "pub struct App;\n\nimpl App {\n    pub fn watch_dir(&self) {}\n}\n\n\
               pub struct Meta;\n\nimpl Meta {\n    pub fn page(&self) {}\n}\n";
    put(&mut store, &language, "src/app.rs", app);
    put(
        &mut store,
        &language,
        "src/other.rs",
        "pub fn watch_dir() {}\n",
    );
    // A method is a row `read_items` never answers, and it is here with
    // its kind, path and line.
    let hits = store.symbols_named("watch_dir").expect("named");
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(hits[0].kind, "method");
    assert_eq!(hits[0].file, "src/app.rs");
    assert!(hits[0].line.is_some());
    assert_eq!(hits[0].visibility, "public");
    assert!(hits[0].id > 0);
    assert_ne!(hits[0].id, hits[1].id);
    assert_eq!(hits[1].kind, "function");
    assert_eq!(hits[1].file, "src/other.rs");
    let page = store.symbols_named("page").expect("page");
    assert_eq!(page.len(), 1, "{page:?}");
    assert_eq!(page[0].kind, "method");
    assert_eq!(store.symbols_named("App").expect("type")[0].kind, "struct");
    assert!(store.symbols_named("nowhere").expect("none").is_empty());
}

#[test]
fn symbols_matching_takes_a_method_by_its_module() {
    let db = TempDb::new("matchmethod");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    let app = "pub struct App;\n\nimpl App {\n    pub fn watch_dir(&self) {}\n}\n\n\
               pub struct Meta;\n\nimpl Meta {\n    pub fn page(&self) {}\n}\n";
    put(&mut store, &language, "src/tui/app.rs", app);
    write_modules(&mut store, &[("crate::tui::app", "src/tui/app.rs")]);
    // The bare name, the type-and-name form, and the module forms from
    // every `::` boundary.
    for spelling in [
        "watch_dir",
        "App::watch_dir",
        "app::watch_dir",
        "tui::app::watch_dir",
        "tui::app::App::watch_dir",
        "crate::app::watch_dir",
        "crate::tui::app::App::watch_dir",
    ] {
        let hits = store.symbols_matching(spelling).expect(spelling);
        assert_eq!(hits.len(), 1, "{spelling}: {hits:?}");
        assert_eq!(hits[0].kind, "method", "{spelling}");
        assert_eq!(hits[0].file, "src/tui/app.rs", "{spelling}");
    }
    // A bare `page` lists the method, and the type-and-name form too.
    let hits = store.symbols_matching("page").expect("page");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].qual_name, "Meta::page");
    assert_eq!(
        store
            .symbols_matching("Meta::page")
            .expect("meta page")
            .len(),
        1
    );
    // A miss offers the last segment's candidates.
    assert!(
        store
            .symbols_matching("wrong::watch_dir")
            .expect("miss")
            .is_empty()
    );
    let candidates = store.did_you_mean("wrong::watch_dir").expect("dym");
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].qual_name, "App::watch_dir");
    assert!(store.did_you_mean("nowhere").expect("none").is_empty());
}

#[test]
fn did_you_mean_stops_at_ten() {
    let db = TempDb::new("dymcap");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    for n in 0..12 {
        put(
            &mut store,
            &language,
            &format!("src/t{n:02}.rs"),
            "pub fn twin() {}\n",
        );
    }
    let candidates = store.did_you_mean("miss::twin").expect("dym");
    assert_eq!(candidates.len(), 10, "{candidates:?}");
}

#[test]
fn a_free_fn_keeps_the_module_spelling_from_a_same_file_method() {
    let db = TempDb::new("dumpshape");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    // The `dump` shape: a free fn and a method of the same name share one
    // file and one module.
    let source = "pub struct Store;\n\nimpl Store {\n    pub fn canonical_dump(&self) {}\n}\n\n\
                  pub fn canonical_dump() {}\n";
    put(&mut store, &language, "src/dump.rs", source);
    write_modules(&mut store, &[("crate::dump", "src/dump.rs")]);
    // The module spelling the base answers keeps exactly its row: the
    // free fn, never the method beside it.
    let hits = store
        .symbols_matching("dump::canonical_dump")
        .expect("module");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].kind, "function");
    assert_eq!(hits[0].qual_name, "canonical_dump");
    // The bare name still lists both, and the type spelling names the
    // method through its exact form.
    let bare = store.symbols_matching("canonical_dump").expect("bare");
    assert_eq!(bare.len(), 2, "{bare:?}");
    let method = store
        .symbols_matching("Store::canonical_dump")
        .expect("type");
    assert_eq!(method.len(), 1, "{method:?}");
    assert_eq!(method[0].kind, "method");
}
#[test]
fn the_display_spelling_names_the_row_it_spells() {
    let db = TempDb::new("displayspell");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    // One `run` per file: the bare name lists three, the display spelling
    // names the ones in the file it gives.
    put(
        &mut store,
        &language,
        "src/app.rs",
        "pub struct App;\n\nimpl App {\n    pub fn run(&self) {}\n}\n",
    );
    put(
        &mut store,
        &language,
        "src/other.rs",
        "pub fn run() {}\n\npub struct Other;\n\nimpl Other {\n    pub fn run(&self) {}\n}\n",
    );
    write_modules(
        &mut store,
        &[
            ("crate::app", "src/app.rs"),
            ("crate::other", "src/other.rs"),
        ],
    );
    // The display spelling names one method, with its file and its line.
    let named = store
        .symbols_matching("src/app.rs::App::run")
        .expect("display method");
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].kind, "method");
    assert_eq!(named[0].qual_name, "App::run");
    assert_eq!(named[0].file, "src/app.rs");
    assert!(named[0].line.is_some());
    // The bare name inside a two-symbol file answers both rows, each with
    // its path: the free fn and the method.
    let both = store
        .symbols_matching("src/other.rs::run")
        .expect("display name");
    assert_eq!(both.len(), 2, "{both:?}");
    let kinds: Vec<&str> = both.iter().map(|hit| hit.kind.as_str()).collect();
    assert_eq!(kinds, ["method", "function"], "{both:?}");
    assert!(both.iter().all(|hit| hit.file == "src/other.rs"));
    // A leading `./` and a file outside the tree answer as spelled.
    assert_eq!(
        store
            .symbols_matching("./src/app.rs::App::run")
            .expect("dot slash")
            .len(),
        1
    );
    assert!(
        store
            .symbols_matching("src/nope.rs::run")
            .expect("no file")
            .is_empty()
    );
    // The forms the base answers keep exactly their rows: the bare name,
    // the type spelling, and a module path whose first segment is no file.
    assert_eq!(store.symbols_matching("run").expect("bare").len(), 3);
    let typed = store.symbols_matching("App::run").expect("type");
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert_eq!(typed[0].file, "src/app.rs");
    assert!(
        store
            .symbols_matching("std::vec::Vec")
            .expect("std")
            .is_empty()
    );
}
#[test]
fn a_display_spelling_takes_the_inline_scope_and_the_shared_file() {
    let db = TempDb::new("displayscope");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    // Two files with one `tests::check` apiece: the in-file scope spelling
    // is ambiguous, and the display spelling names each.
    for file in ["src/a.rs", "src/b.rs"] {
        put(
            &mut store,
            &language,
            file,
            "mod tests {\n    pub fn check() {}\n}\n",
        );
    }
    write_modules(
        &mut store,
        &[("crate::a", "src/a.rs"), ("crate::b", "src/b.rs")],
    );
    let scoped = store.symbols_matching("tests::check").expect("scope");
    assert_eq!(scoped.len(), 2, "{scoped:?}");
    let display = store
        .symbols_matching("src/b.rs::tests::check")
        .expect("display scope");
    assert_eq!(display.len(), 1, "{display:?}");
    assert_eq!(display[0].file, "src/b.rs");
    assert_eq!(display[0].qual_name, "tests::check");
    // The `dump` shape: a display spelling with the free fn's bare name
    // still answers the method beside it, and the module spelling the base
    // answers keeps exactly its row.
    put(
        &mut store,
        &language,
        "src/dump.rs",
        "pub struct Store;\n\nimpl Store {\n    pub fn canonical_dump(&self) {}\n}\n\n\
         pub fn canonical_dump() {}\n",
    );
    write_modules(&mut store, &[("crate::dump", "src/dump.rs")]);
    let dump = store
        .symbols_matching("src/dump.rs::canonical_dump")
        .expect("display dump");
    assert_eq!(dump.len(), 2, "{dump:?}");
    let module = store
        .symbols_matching("dump::canonical_dump")
        .expect("module");
    assert_eq!(module.len(), 1, "{module:?}");
    assert_eq!(module[0].kind, "function");
}

#[test]
fn the_display_spelling_needs_a_file_shaped_prefix() {
    let db = TempDb::new("displayshape");
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = language();
    // A dot with no slash, a slash with no dot, and a bare word with
    // neither: each file holds one function `go`.
    for path in ["build.rs", "bin/tool", "notes"] {
        put(&mut store, &language, path, "pub fn go() {}\n");
    }
    for spelled in ["build.rs::go", "bin/tool::go"] {
        let hits = store.symbols_matching(spelled).expect("display");
        assert_eq!(hits.len(), 1, "{spelled}: {hits:?}");
    }
    // `notes` is a module-shaped word, not a path: the form is not tried.
    let bare = store.symbols_matching("notes::go").expect("bare word");
    assert!(bare.is_empty(), "{bare:?}");
    // An empty name after the separator names nothing.
    let empty = store.symbols_matching("build.rs::").expect("empty name");
    assert!(empty.is_empty(), "{empty:?}");
}
