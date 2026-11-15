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
