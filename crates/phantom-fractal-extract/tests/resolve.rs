//! The index pass over `fixtures/resolve`, checked against its
//! `EXPECTED.md` (SPEC-w3 G2c, G2d).

mod common;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use common::{TempDir, copy_tree};
use phantom_fractal_extract::{EXTRACTOR_VERSION, GraphReport, IndexReport, index};
use phantom_fractal_lang::{Id, ResolveInput, SymbolKind};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/resolve");
const EXPECTED: &str = include_str!("../../../fixtures/resolve/EXPECTED.md");
const TABLES: [&str; 7] = [
    "files", "symbols", "refs", "edges", "modules", "targets", "packages",
];

struct Setup {
    repo: TempDir,
    db_dir: TempDir,
    store: Store,
    rust: RustLanguage,
}

fn setup() -> Setup {
    let repo = TempDir::new("resolve");
    copy_tree(Path::new(FIXTURE), repo.path());
    let db_dir = TempDir::new("db");
    let store = Store::open(&db_dir.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    Setup {
        repo,
        db_dir,
        store,
        rust,
    }
}

fn run(s: &mut Setup) -> IndexReport {
    index(s.repo.path(), &mut s.store, &s.rust).expect("index")
}

/// `EXPECTED.md`'s edge lines, sorted, and its counts.
fn expected() -> (Vec<String>, HashMap<String, u64>) {
    let mut edges = Vec::new();
    let mut counts = HashMap::new();
    let mut section = "";
    for line in EXPECTED.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading;
            continue;
        }
        let Some(entry) = line.strip_prefix("- ") else {
            continue;
        };
        match section {
            "Edges" => edges.push(entry.trim_matches('`').to_owned()),
            "Counts" => {
                let (key, value) = entry.split_once(": ").expect("count line");
                counts.insert(key.to_owned(), value.parse().expect("count value"));
            }
            _ => {}
        }
    }
    edges.sort();
    (edges, counts)
}

/// `<target>:<module path>::<owner>::<name>` for each symbol, where the
/// target is its root file's stem, with `@<start line>` when another
/// symbol in the same module has the same owner and name.
fn labels(store: &Store, input: &ResolveInput) -> HashMap<Id, String> {
    let targets: HashMap<Id, String> = input
        .targets
        .iter()
        .map(|t| {
            let stem = Path::new(&t.root).file_stem().and_then(|s| s.to_str());
            (t.id, stem.unwrap_or_default().to_owned())
        })
        .collect();
    let modules: HashMap<Id, (String, Id)> = input
        .modules
        .iter()
        .map(|m| (m.id, (m.path.clone(), m.target)))
        .collect();
    input
        .symbols
        .iter()
        .map(|symbol| {
            let place = match symbol.module.and_then(|id| modules.get(&id)) {
                Some((path, target)) => format!("{}:{path}", targets[target]),
                None => format!("orphan:{}", symbol.file),
            };
            let mut name = match (&symbol.kind, &symbol.owner) {
                (SymbolKind::Method, Some(owner)) => format!("{owner}::{}", symbol.name),
                _ => symbol.name.clone(),
            };
            let twins = input.symbols.iter().filter(|other| {
                other.module == symbol.module
                    && other.file == symbol.file
                    && other.owner == symbol.owner
                    && other.name == symbol.name
            });
            if twins.count() > 1 {
                let line = store
                    .symbols_matching(&symbol.name)
                    .expect("symbols matching")
                    .into_iter()
                    .find(|found| found.id == symbol.id)
                    .and_then(|found| found.line)
                    .expect("twin line");
                name = format!("{name}@{line}");
            }
            (symbol.id, format!("{place}::{name}"))
        })
        .collect()
}

/// Every stored edge as `caller -> callee reason`, sorted. Each edge's
/// confidence is checked against its reason.
fn stored_edges(store: &Store) -> Vec<String> {
    let input = store.resolve_input().expect("resolve input");
    let labels = labels(store, &input);
    let mut out = Vec::new();
    for symbol in &input.symbols {
        for hit in store.callees(symbol.id).expect("callees") {
            let confidence = match hit.reason.as_str() {
                "path-exact" => 1.0,
                "local" | "use-alias" => 0.95,
                "assoc" => 0.9,
                other => panic!("unknown reason {other}"),
            };
            assert!((hit.confidence - confidence).abs() < 1e-9, "{hit:?}");
            out.push(format!(
                "{} -> {} {}",
                labels[&symbol.id], labels[&hit.symbol], hit.reason
            ));
        }
    }
    out.sort();
    out
}

fn row_counts(store: &Store) -> Vec<i64> {
    TABLES
        .iter()
        .map(|table| store.row_count(table).expect("count"))
        .collect()
}

fn graph(report: &IndexReport) -> GraphReport {
    report.graph.expect("graph rebuilt")
}

#[test]
fn fixture_edges_and_counts_match_expected_exactly() {
    let mut s = setup();
    let report = run(&mut s);
    let (edges, counts) = expected();
    assert_eq!(stored_edges(&s.store), edges);
    let graph = graph(&report);
    let got = HashMap::from([
        ("edges", graph.counts.edges),
        ("unresolved", graph.counts.unresolved),
        ("external", graph.counts.external),
        ("no_source", graph.counts.no_source),
        ("method_calls", graph.counts.method_calls),
        ("macros", graph.counts.macros),
        ("modules", graph.modules as u64),
        ("orphans", graph.orphans as u64),
    ]);
    assert_eq!(got.len(), counts.len());
    for (key, want) in &counts {
        assert_eq!(got.get(key.as_str()), Some(want), "{key}");
    }
    assert_eq!(
        s.store.extractor_version().expect("version"),
        Some(EXTRACTOR_VERSION)
    );
}

#[test]
fn a_second_pass_over_an_unchanged_tree_writes_nothing() {
    let mut s = setup();
    run(&mut s);
    let edges = stored_edges(&s.store);
    let rows = row_counts(&s.store);
    let db = s.db_dir.path().join("graph.db");
    let before = fs::read(&db).expect("read db");
    let report = run(&mut s);
    assert!(report.written.is_empty(), "{:?}", report.written);
    assert!(report.removed.is_empty(), "{:?}", report.removed);
    assert_eq!(report.graph, None);
    assert_eq!(report.unchanged.len(), 11);
    assert_eq!(row_counts(&s.store), rows);
    assert_eq!(stored_edges(&s.store), edges);
    assert_eq!(fs::read(&db).expect("read db"), before);
}

#[test]
fn a_database_at_extractor_version_1_is_fully_reextracted() {
    let mut s = setup();
    let first = run(&mut s);
    s.store.set_extractor_version(1).expect("set version");
    let report = run(&mut s);
    let mut written = report.written.clone();
    written.sort();
    let mut all = first.written.clone();
    all.sort();
    assert_eq!(written.len(), 11);
    assert_eq!(written, all);
    assert!(report.unchanged.is_empty());
    assert!(report.graph.is_some());
    assert_eq!(s.store.extractor_version().expect("version"), Some(2));
    assert_eq!(stored_edges(&s.store), expected().0);
}

#[test]
fn a_deleted_file_loses_its_rows_and_edges() {
    let mut s = setup();
    run(&mut s);
    fs::remove_file(s.repo.path().join("src/util/deep.rs")).expect("delete");
    let report = run(&mut s);
    assert_eq!(report.removed, ["src/util/deep.rs"]);
    assert!(
        !s.store
            .file_paths()
            .expect("paths")
            .contains(&"src/util/deep.rs".to_owned())
    );
    assert!(
        s.store
            .read_items("src/util/deep.rs")
            .expect("items")
            .is_empty()
    );
    let edges = stored_edges(&s.store);
    assert!(!edges.iter().any(|edge| edge.contains("sink")), "{edges:?}");
    let graph = graph(&report);
    assert_eq!(graph.counts.edges, 20);
    assert_eq!(graph.modules, 10);
    // `deep::sink()` in `util::helper` now names a module with no file.
    assert_eq!(graph.counts.unresolved, 9);
}

#[test]
fn an_edited_call_moves_its_edge() {
    let mut s = setup();
    run(&mut s);
    let cli = s.repo.path().join("src/cli.rs");
    let text = fs::read_to_string(&cli).expect("read cli");
    fs::write(
        &cli,
        text.replace("    banner(); // local\n", "    crate::cli::banner();\n"),
    )
    .expect("edit cli");
    let report = run(&mut s);
    assert_eq!(report.written, ["src/cli.rs"]);
    let edges = stored_edges(&s.store);
    assert!(
        edges.contains(&"main:crate::cli::run -> main:crate::cli::banner path-exact".to_owned())
    );
    assert!(!edges.contains(&"main:crate::cli::run -> main:crate::cli::banner local".to_owned()));
}
