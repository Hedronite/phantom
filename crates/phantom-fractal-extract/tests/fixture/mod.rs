//! Index a copy of one fixture package and check its edges and counts
//! against the fixture's `EXPECTED.md`.
// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use phantom_fractal_extract::{GraphReport, IndexReport, ReasonCounts, index};
use phantom_fractal_lang::{Id, Reason, ResolveInput, SymbolKind};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

use crate::common::{TempDir, copy_tree};

pub const TABLES: [&str; 7] = [
    "files", "symbols", "refs", "edges", "modules", "targets", "packages",
];

pub struct Setup {
    pub repo: TempDir,
    pub db_dir: TempDir,
    pub store: Store,
    pub rust: RustLanguage,
}

/// A fresh store and a temp copy of `fixture`.
pub fn setup(fixture: &str) -> Setup {
    let repo = TempDir::new("resolve");
    copy_tree(Path::new(fixture), repo.path());
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

pub fn run(s: &mut Setup) -> IndexReport {
    index(s.repo.path(), &mut s.store, &s.rust).expect("index")
}

/// The store's text for `reason`, and its confidence.
pub fn reason_text(reason: Reason) -> (&'static str, f64) {
    match reason {
        Reason::PathExact => ("path-exact", 1.0),
        Reason::Local => ("local", 0.95),
        Reason::UseAlias => ("use-alias", 0.95),
        Reason::Assoc => ("assoc", 0.9),
        Reason::SelfMethod => ("self-method", 0.9),
        Reason::SelfAssoc => ("self-assoc", 0.9),
        Reason::CratePath => ("crate-path", 0.95),
    }
}

/// An `EXPECTED.md`'s edge lines, sorted, and its counts.
pub fn expected(text: &str) -> (Vec<String>, HashMap<String, u64>) {
    let mut edges = Vec::new();
    let mut counts = HashMap::new();
    let mut section = "";
    for line in text.lines() {
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
/// symbol in the same module has the same owner and name. With more than
/// one package, the target is `<package>/<stem>`.
pub fn labels(store: &Store, input: &ResolveInput) -> HashMap<Id, String> {
    let first = input.targets.first().map(|t| t.package.as_str());
    let many = input
        .targets
        .iter()
        .any(|t| Some(t.package.as_str()) != first);
    let targets: HashMap<Id, String> = input
        .targets
        .iter()
        .map(|t| {
            let stem = Path::new(&t.root).file_stem().and_then(|s| s.to_str());
            let stem = stem.unwrap_or_default();
            let label = if many {
                format!("{}/{stem}", t.package)
            } else {
                stem.to_owned()
            };
            (t.id, label)
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
pub fn stored_edges(store: &Store) -> Vec<String> {
    let input = store.resolve_input().expect("resolve input");
    let labels = labels(store, &input);
    let mut out = Vec::new();
    for symbol in &input.symbols {
        for hit in store.callees(symbol.id).expect("callees") {
            let (_, confidence) = ReasonCounts::REASONS
                .into_iter()
                .map(reason_text)
                .find(|(text, _)| *text == hit.reason)
                .unwrap_or_else(|| panic!("unknown reason {}", hit.reason));
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

pub fn row_counts(store: &Store) -> Vec<i64> {
    TABLES
        .iter()
        .map(|table| store.row_count(table).expect("count"))
        .collect()
}

pub fn graph(report: &IndexReport) -> GraphReport {
    report.graph.expect("graph rebuilt")
}

/// Asserts that the stored edges equal `expected_text`'s **Edges** exactly,
/// that every **Counts** line matches `graph`, and that the per-reason
/// counts equal the reasons the **Edges** lines name.
pub fn assert_matches_expected(store: &Store, graph: &GraphReport, expected_text: &str) {
    let (edges, counts) = expected(expected_text);
    assert_eq!(stored_edges(store), edges);
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
    let mut total = 0;
    for reason in ReasonCounts::REASONS {
        let (text, _) = reason_text(reason);
        let suffix = format!(" {text}");
        let want = edges.iter().filter(|edge| edge.ends_with(&suffix)).count() as u64;
        assert_eq!(graph.edges_by_reason.get(reason), want, "{text}");
        total += want;
    }
    assert_eq!(total, graph.counts.edges);
}
