//! Every row-mapping read answers the rows an index pass wrote (W13 G1):
//! the resolver's input, the rank and map rows, the per-symbol detail
//! and impact reads, the scorer's reads and the edge neighbours. One
//! fixture writes a crate graph over four files with two packages, a
//! lib, a bin and a test target, a method owner, a call edge, two
//! import links, an impl link, a rank and a co-change scan; each test
//! reads one family back.
use super::{TempDb, assert_outside_workspace};
use crate::{CochangeScan, FileRow, Package, PairRow, Store};
use phantom_fractal_lang::{
    Edge, Id, Link, LinkKind, LinkReason, Module, Node, Reason, RefKind, SymbolKind, Target,
    TargetKind,
};
use phantom_fractal_lang_rust::RustLanguage;
const LIB: &str = "\
pub mod helper;
pub use helper::*;
use helper::three as triplet;
pub fn one() {
    three();
}
pub fn two() {
    let _ = triplet();
    assert_eq!(one(), ());
}
pub struct Holder {
    field: u8,
}
impl Holder {
    pub fn method(&self) {
        two();
    }
}
pub trait Shape {}
impl Shape for Holder {}
";
/// The whole fixture: the store it wrote, the ids its rows carry, and
/// the temp database that owns them.
struct Fixture {
    _db: TempDb,
    store: Store,
    one: Id,
    two: Id,
    three: Id,
    holder: Id,
    shape: Id,
    method: Id,
    lib_file: i64,
}
/// One co-change scan over two of the four files, so a pair and a
/// rename read back.
fn scan() -> CochangeScan {
    CochangeScan {
        last_oid: Some("1".repeat(40)),
        oldest_oid: Some("1".repeat(40)),
        commits_seen: 3,
        skipped_bulk: 1,
        window_json: "{}".to_owned(),
        files: vec![
            FileRow {
                path: "src/lib.rs".to_owned(),
                commits: 2.0,
                last_time: 30,
                renamed_from: Some("src/old.rs".to_owned()),
            },
            FileRow {
                path: "src/helper.rs".to_owned(),
                commits: 1.0,
                last_time: 20,
                renamed_from: None,
            },
        ],
        pairs: vec![PairRow {
            a: "src/lib.rs".to_owned(),
            b: "src/helper.rs".to_owned(),
            together: 1.0,
            n_a: 2.0,
            n_b: 1.0,
            lift: 1.5,
            conf_ab: 0.5,
            conf_ba: 0.25,
            last_oid: "1".repeat(40),
            last_time: 30,
        }],
    }
}
fn populated(label: &str) -> Fixture {
    let db = TempDb::new(label);
    assert_outside_workspace(db.path());
    let mut store = Store::open(db.path()).expect("open");
    let language = RustLanguage::new().expect("grammar");
    store
        .put_source("src/lib.rs", LIB, 1, &language)
        .expect("put lib");
    store
        .put_source("src/helper.rs", "pub fn three() {}\n", 1, &language)
        .expect("put helper");
    store
        .put_source("src/main.rs", "fn main() {\n    one();\n}\n", 1, &language)
        .expect("put bin");
    store
        .put_source("tests/common/mod.rs", "pub fn shared() {}\n", 1, &language)
        .expect("put shared");
    let packages = [
        Package {
            name: "sample".to_owned(),
            version: Some("0.0.0".to_owned()),
            manifest_path: "/w/Cargo.toml".to_owned(),
        },
        Package {
            name: "nested".to_owned(),
            version: None,
            manifest_path: "/w/vendor/nested/Cargo.toml".to_owned(),
        },
    ];
    let targets = [
        Target {
            id: 1,
            package: "sample".to_owned(),
            name: "sample".to_owned(),
            kind: TargetKind::Lib,
            root: "src/lib.rs".to_owned(),
        },
        Target {
            id: 2,
            package: "sample".to_owned(),
            name: "sample".to_owned(),
            kind: TargetKind::Bin,
            root: "src/main.rs".to_owned(),
        },
        Target {
            id: 3,
            package: "sample".to_owned(),
            name: "shared".to_owned(),
            kind: TargetKind::Test,
            root: "tests/common/mod.rs".to_owned(),
        },
        Target {
            id: 4,
            package: "nested".to_owned(),
            name: "nested".to_owned(),
            kind: TargetKind::Lib,
            root: "vendor/nested/src/lib.rs".to_owned(),
        },
    ];
    let modules = [
        Module {
            id: 10,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: "src/lib.rs".to_owned(),
            scope: String::new(),
        },
        Module {
            id: 11,
            target: 1,
            parent: Some(10),
            path: "crate::helper".to_owned(),
            file: "src/helper.rs".to_owned(),
            scope: String::new(),
        },
        Module {
            id: 12,
            target: 2,
            parent: None,
            path: "crate".to_owned(),
            file: "src/main.rs".to_owned(),
            scope: String::new(),
        },
        Module {
            id: 13,
            target: 3,
            parent: None,
            path: "crate".to_owned(),
            file: "tests/common/mod.rs".to_owned(),
            scope: String::new(),
        },
    ];
    // The test target's module is the only test mark: its file's
    // symbols are tests, every other file's are not.
    store
        .write_crate_graph(&packages, &targets, &modules, &[13])
        .expect("graph");
    let input = store.resolve_input().expect("input");
    let id_of = |name: &str, file: &str| -> Id {
        input
            .symbols
            .iter()
            .find(|symbol| symbol.name == name && symbol.file == file)
            .unwrap_or_else(|| panic!("no symbol {name} in {file}"))
            .id
    };
    let one = id_of("one", "src/lib.rs");
    let two = id_of("two", "src/lib.rs");
    let three = id_of("three", "src/helper.rs");
    let holder = id_of("Holder", "src/lib.rs");
    let shape = id_of("Shape", "src/lib.rs");
    let method = id_of("method", "src/lib.rs");
    let call = input
        .refs
        .iter()
        .find(|call| call.kind == RefKind::Call && call.text == "three")
        .expect("the call ref")
        .id;
    store
        .replace_edges(&[Edge {
            ref_id: call,
            source: one,
            target: three,
            confidence: 0.9,
            reason: Reason::PathExact,
        }])
        .expect("edges");
    store
        .write_method_owners(&[(method, holder)])
        .expect("owners");
    let uses = store.use_rows().expect("uses");
    let alias = uses
        .iter()
        .find(|row| row.alias.as_deref() == Some("triplet"))
        .expect("the alias use")
        .id;
    let glob = uses.iter().find(|row| row.glob).expect("the glob use").id;
    let module_rows = store.module_rows().expect("modules");
    let lib_root = module_rows
        .iter()
        .find(|(_, row)| row.target_id == 1 && row.path == "crate")
        .map(|(id, _)| *id)
        .expect("the lib root module");
    let helper_module = module_rows
        .iter()
        .find(|(_, row)| row.path == "crate::helper")
        .map(|(id, _)| *id)
        .expect("the helper module");
    store
        .write_links(&[
            Link {
                kind: LinkKind::Import,
                source: Node::Module(lib_root),
                target: Node::Symbol(three),
                confidence: 0.9,
                reason: LinkReason::UseAlias,
                ref_id: Some(alias),
            },
            Link {
                kind: LinkKind::Import,
                source: Node::Module(lib_root),
                target: Node::Module(helper_module),
                confidence: 0.8,
                reason: LinkReason::UseAlias,
                ref_id: Some(glob),
            },
            Link {
                kind: LinkKind::Impl,
                source: Node::Symbol(holder),
                target: Node::Symbol(shape),
                confidence: 0.7,
                reason: LinkReason::ImplTrait,
                ref_id: None,
            },
        ])
        .expect("links");
    let lib_file = store
        .files()
        .expect("files")
        .into_iter()
        .find(|file| file.path == "src/lib.rs")
        .expect("the lib file")
        .id;
    store
        .write_rank(2, &[(one, 1.0), (three, 0.5)], &[(lib_file, 0.25)])
        .expect("rank");
    store.write_cochange(&scan()).expect("cochange");
    Fixture {
        _db: db,
        store,
        one,
        two,
        three,
        holder,
        shape,
        method,
        lib_file,
    }
}
#[test]
fn resolve_input_carries_every_stored_row() {
    let fixture = populated("readarms");
    let input = fixture.store.resolve_input().expect("input");
    // Targets: every row with its package and kind text.
    assert_eq!(input.targets.len(), 4);
    let bin = input
        .targets
        .iter()
        .find(|target| target.kind == TargetKind::Bin)
        .expect("the bin target");
    assert_eq!(bin.root, "src/main.rs");
    assert_eq!(bin.package, "sample");
    assert_eq!(
        input
            .targets
            .iter()
            .find(|target| target.kind == TargetKind::Test)
            .expect("the test target")
            .name,
        "shared"
    );
    assert_eq!(
        input
            .targets
            .iter()
            .find(|target| target.package == "nested")
            .expect("the nested target")
            .kind,
        TargetKind::Lib
    );
    // Modules: the parent link and the file each one claims.
    let lib_target = input
        .targets
        .iter()
        .find(|target| target.kind == TargetKind::Lib && target.package == "sample")
        .expect("the lib target");
    let root = input
        .modules
        .iter()
        .find(|module| module.target == lib_target.id && module.path == "crate")
        .expect("the lib root module");
    let helper = input
        .modules
        .iter()
        .find(|module| module.path == "crate::helper")
        .expect("the helper module");
    assert_eq!(helper.parent, Some(root.id));
    assert_eq!(helper.file, "src/helper.rs");
    // Symbols: kinds decode back, and the owner and module are stored.
    let method = input
        .symbols
        .iter()
        .find(|symbol| symbol.id == fixture.method)
        .expect("the method symbol");
    assert_eq!(method.kind, SymbolKind::Method);
    assert_eq!(method.name, "method");
    assert_eq!(method.module, Some(helper.parent.expect("lib root")));
    assert_eq!(
        input
            .symbols
            .iter()
            .find(|symbol| symbol.id == fixture.holder)
            .expect("the holder")
            .kind,
        SymbolKind::Struct
    );
    assert_eq!(
        input
            .symbols
            .iter()
            .find(|symbol| symbol.id == fixture.shape)
            .expect("the trait")
            .kind,
        SymbolKind::Trait
    );
    // Uses: the glob's path stops before `*`, and the alias rides.
    assert!(
        input
            .uses
            .iter()
            .any(|binding| binding.glob && binding.path == "helper")
    );
    assert!(
        input
            .uses
            .iter()
            .any(|binding| binding.alias.as_deref() == Some("triplet"))
    );
    // Call refs: the path call the edge resolved, and the macro argument.
    let call = input
        .refs
        .iter()
        .find(|call| call.kind == RefKind::Call && call.text == "three")
        .expect("the call ref");
    assert_eq!(call.from, Some(fixture.one));
    assert_eq!(call.file, "src/lib.rs");
    assert!(input.refs.iter().any(|call| call.kind == RefKind::Macro));
}
#[test]
fn rank_and_map_reads_answer_a_ranked_store() {
    let fixture = populated("readrank");
    let rank_input = fixture.store.rank_input().expect("rank input");
    // `fn main` in a bin's root file is the one main row.
    let mains: Vec<_> = rank_input
        .iter()
        .filter(|row| row.is_main)
        .map(|row| row.path.as_str())
        .collect();
    assert_eq!(mains, vec!["src/main.rs"]);
    let main = rank_input
        .iter()
        .find(|row| row.is_main)
        .expect("the main row");
    assert_eq!(main.target_kind.as_deref(), Some("bin"));
    let one = rank_input
        .iter()
        .find(|row| row.id == fixture.one)
        .expect("the `one` row");
    assert_eq!(one.target_kind.as_deref(), Some("lib"));
    assert_eq!(one.kind, "function");
    assert_eq!(one.visibility, "public");
    assert!(!one.is_test);
    // The test target's file carries the test mark.
    assert!(
        rank_input
            .iter()
            .any(|row| row.path == "tests/common/mod.rs" && row.is_test)
    );
    // Call and impl edges ship their kind and confidence.
    let calls = fixture.store.rank_call_edges().expect("call edges");
    let call = calls
        .iter()
        .find(|edge| edge.kind == "call")
        .expect("the call edge");
    assert_eq!((call.src_id, call.dst_id), (fixture.one, fixture.three));
    assert!((call.confidence - 0.9).abs() < f64::EPSILON);
    assert!(calls.iter().any(|edge| edge.kind == "impl"));
    // Import edges land on files: one symbol end, one module end.
    let imports = fixture.store.rank_import_edges().expect("import edges");
    assert_eq!(imports.len(), 2);
    assert!(
        imports
            .iter()
            .all(|edge| edge.src_file_id == fixture.lib_file)
    );
    assert!(
        imports
            .iter()
            .filter(|edge| edge.dst_file_id != fixture.lib_file)
            .count()
            == 2
    );
    // The stored rank table keeps both node kinds.
    let stored = fixture.store.stored_rank().expect("stored rank");
    assert_eq!(stored.len(), 3);
    assert!(
        stored
            .iter()
            .any(|(kind, id, _)| kind == "file" && *id == fixture.lib_file)
    );
    // Files and map rows: the map drops modules and tests.
    let files = fixture.store.files().expect("files");
    assert_eq!(files.len(), 4);
    assert_eq!(
        fixture.store.map_file_paths().expect("paths"),
        vec![
            "src/helper.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "src/main.rs".to_owned(),
            "tests/common/mod.rs".to_owned()
        ]
    );
    let rows = fixture.store.map_rows().expect("map rows");
    let method = rows
        .iter()
        .find(|row| row.id == fixture.method)
        .expect("the method row");
    assert!(method.owner.is_some());
    assert!(method.start_line.is_some());
    assert!(rows.iter().all(|row| row.kind != "module"));
    assert!(rows.iter().all(|row| row.file_id != files[3].id));
    // The epochs the readers keep: the graph's moved once, the rank's
    // carries the write.
    assert_eq!(fixture.store.graph_epoch().expect("graph epoch"), 1);
    assert_eq!(fixture.store.rank_epoch().expect("rank epoch"), 2);
}
#[test]
fn detail_reads_answer_their_rows() {
    let fixture = populated("readdetail");
    let detail = fixture
        .store
        .symbol_detail(fixture.holder)
        .expect("detail")
        .expect("the holder row");
    assert_eq!(detail.name, "Holder");
    assert_eq!(detail.kind, "struct");
    assert_eq!(detail.path, "src/lib.rs");
    assert!(detail.start_line.is_some());
    assert!(!detail.is_test);
    assert!(fixture.store.symbol_detail(9_999).expect("miss").is_none());
    // Members: the method the owner write attributed to the struct.
    let members = fixture.store.members(fixture.holder).expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].name, "method");
    assert_eq!(members[0].id, fixture.method);
    // A function and a trait carry no members.
    assert!(fixture.store.members(fixture.one).expect("fn").is_empty());
    assert!(
        fixture
            .store
            .members(fixture.shape)
            .expect("trait")
            .is_empty()
    );
    // The stable key names the row back.
    let key = detail.stable_key.clone();
    assert_eq!(
        fixture.store.symbol_by_stable_key(&key).expect("by key"),
        Some(fixture.holder)
    );
    assert!(
        fixture
            .store
            .symbol_by_stable_key("no-such-key")
            .expect("miss")
            .is_none()
    );
}
#[test]
fn impact_reads_answer_their_rows() {
    let fixture = populated("readimpact");
    let facts = fixture
        .store
        .symbol_facts(fixture.one)
        .expect("facts")
        .expect("the `one` row");
    assert_eq!(facts.name, "one");
    assert_eq!(facts.kind, "function");
    assert_eq!(facts.file, "src/lib.rs");
    assert_eq!(facts.package.as_deref(), Some("sample"));
    assert_eq!(facts.target_kind.as_deref(), Some("lib"));
    assert_eq!(facts.trait_name, None);
    assert!(!facts.is_test);
    assert!(fixture.store.symbol_facts(9_999).expect("miss").is_none());
    // A trait method keeps its trait text; the test file's symbols are
    // marked.
    assert!(
        fixture
            .store
            .symbol_spans()
            .expect("spans")
            .iter()
            .any(|span| span.path == "tests/common/mod.rs" && span.is_test)
    );
    let span = fixture
        .store
        .symbol_spans()
        .expect("spans")
        .into_iter()
        .find(|span| span.id == fixture.one)
        .expect("the `one` span");
    assert_eq!(span.file_id, fixture.lib_file);
    assert!(span.end_line >= span.start_line);
    // The placed call and the unplaced ones.
    let refs = fixture.store.symbol_call_refs(fixture.one).expect("refs");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].kind, "call");
    assert_eq!(refs[0].text, "three");
    assert!(refs[0].placed);
    assert_eq!(fixture.store.unresolved_named("three").expect("named"), 0);
    assert_eq!(fixture.store.unresolved_named("triplet").expect("named"), 1);
    // The unplaced call and the macro invocation both count.
    assert_eq!(
        fixture.store.unresolved_calls(fixture.two).expect("calls"),
        2
    );
    assert_eq!(
        fixture.store.unresolved_calls(fixture.one).expect("calls"),
        0
    );
}
#[test]
fn scoring_reads_answer_their_rows() {
    let fixture = populated("readscoring");
    // Every package's own directory, nested vendor package included.
    let dirs = fixture.store.package_dirs().expect("package dirs");
    assert!(dirs.contains("/w"));
    assert!(dirs.contains("/w/vendor/nested"));
    // The scored module rows: one per file module, with its target.
    let modules = fixture.store.module_rows().expect("modules");
    assert_eq!(modules.len(), 4);
    assert!(
        modules
            .values()
            .any(|row| row.path == "crate::helper" && row.file_id.is_some())
    );
    // The scored uses: the glob and the alias.
    let uses = fixture.store.use_rows().expect("uses");
    assert!(uses.iter().any(|row| row.glob && row.text == "helper"));
    assert!(
        uses.iter()
            .any(|row| row.alias.as_deref() == Some("triplet"))
    );
    // The scored import targets, and the module of every symbol.
    let imports = fixture.store.import_targets().expect("imports");
    assert_eq!(imports.len(), 2);
    let modules_of = fixture.store.symbol_modules().expect("symbol modules");
    assert!(modules_of.contains_key(&fixture.three));
    // Ref ids by kind: the macro arguments exist, an unused kind is
    // empty.
    assert!(
        !fixture
            .store
            .ref_ids_of_kind("call")
            .expect("calls")
            .is_empty()
    );
    assert!(
        fixture
            .store
            .ref_ids_of_kind("no-such-kind")
            .expect("none")
            .is_empty()
    );
    // The scorer's call refs keep the receiver and the placement.
    let calls = fixture
        .store
        .score_call_refs(fixture.two)
        .expect("call refs");
    assert!(
        calls
            .iter()
            .any(|call| call.kind == "method_call" || call.kind == "call")
    );
    assert!(calls.iter().all(|call| !call.placed));
    // The same read marks a call an edge placed.
    let placed = fixture
        .store
        .score_call_refs(fixture.one)
        .expect("call refs");
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].text, "three");
    assert!(placed[0].placed);
}
#[test]
fn edge_reads_answer_their_rows() {
    let fixture = populated("readedges");
    let calls = fixture.store.edges_of_kind("call").expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].src_kind, "symbol");
    assert_eq!(calls[0].reason, "path-exact");
    assert!(calls[0].ref_id.is_some());
    let impls = fixture.store.edges_of_kind("impl").expect("impls");
    assert_eq!(impls.len(), 1);
    assert_eq!(impls[0].reason, "impl-trait");
    assert_eq!(impls[0].ref_id, None);
    let imports = fixture.store.edges_of_kind("import").expect("imports");
    assert_eq!(imports.len(), 2);
    assert!(imports.iter().all(|edge| edge.src_kind == "module"));
    // The neighbours: upward to the caller, downward to the callee, both
    // with the file, line, confidence and reason.
    let callers = fixture.store.callers(fixture.three).expect("callers");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].symbol, fixture.one);
    assert_eq!(callers[0].file, "src/lib.rs");
    assert_eq!(callers[0].reason, "path-exact");
    let callees = fixture.store.callees(fixture.one).expect("callees");
    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].symbol, fixture.three);
    assert_eq!(callees[0].file, "src/helper.rs");
    // A symbol with no edges answers empty vectors.
    assert!(fixture.store.callers(fixture.two).expect("none").is_empty());
    assert!(
        fixture
            .store
            .callees(fixture.three)
            .expect("none")
            .is_empty()
    );
    // Lookups by name and by module path both name the row.
    assert!(
        fixture
            .store
            .symbols_matching("three")
            .expect("by name")
            .iter()
            .any(|row| row.id == fixture.three)
    );
    assert!(
        fixture
            .store
            .symbols_matching("crate::helper::three")
            .expect("by path")
            .iter()
            .any(|row| row.id == fixture.three)
    );
    assert!(
        fixture
            .store
            .symbols_matching("no-such-symbol")
            .expect("miss")
            .is_empty()
    );
}
