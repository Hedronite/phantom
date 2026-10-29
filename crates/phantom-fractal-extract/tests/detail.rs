//! `fixtures/detail` against its `EXPECTED.md`: the end line, signature,
//! and doc summary of every item and method (SPEC-w6 G0a).

use std::path::Path;

use phantom_fractal_extract::{Source, read_source, walk};
use phantom_fractal_lang::Language;
use phantom_fractal_lang_rust::RustLanguage;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/detail");
const EXPECTED: &str = include_str!("../../../fixtures/detail/EXPECTED.md");

/// One item or method, in `EXPECTED.md` terms.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    path: String,
    /// `KIND NAME`, with `Owner::name` for a method.
    head: String,
    line: u32,
    end_line: u32,
    signature: String,
    doc_summary: Option<String>,
}

fn expected_rows(text: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut path = None;
    for line in text.lines() {
        if let Some(file) = line.strip_prefix("## File ") {
            path = Some(file.trim().to_owned());
        } else if line.starts_with("## ") {
            path = None;
        } else if let (Some(path), Some(head)) = (&path, line.strip_prefix("### ")) {
            rows.push(Row {
                path: path.clone(),
                head: head.trim().to_owned(),
                line: 0,
                end_line: 0,
                signature: String::new(),
                doc_summary: None,
            });
        } else if let (Some(_), Some(row), Some(entry)) =
            (&path, rows.last_mut(), line.strip_prefix("- "))
        {
            let (key, value) = entry.split_once(": ").expect("key: value");
            match key {
                "line" => row.line = value.parse().expect("line"),
                "end_line" => row.end_line = value.parse().expect("end_line"),
                "signature" => row.signature = value.to_owned(),
                "doc_summary" => row.doc_summary = (value != "none").then(|| value.to_owned()),
                other => panic!("unknown key {other}"),
            }
        }
    }
    rows
}

fn extracted_rows() -> Vec<Row> {
    let rust = RustLanguage::new().expect("query compiles");
    let found = walk(Path::new(FIXTURE)).expect("walk");
    assert!(found.skipped.is_empty(), "{:?}", found.skipped);
    let mut rows = Vec::new();
    for file in found.files.iter().filter(|file| file.path.ends_with(".rs")) {
        let Source::Bytes { bytes, .. } = read_source(&file.abs_path).expect("read") else {
            panic!("{} skipped", file.path);
        };
        let source = String::from_utf8(bytes).expect("utf-8");
        let extract = rust.extract(&source).expect("extract");
        for item in extract.items {
            rows.push(Row {
                path: file.path.clone(),
                head: format!("{} {}", item.kind.as_str(), item.name),
                line: item.start_line,
                end_line: item.end_line,
                signature: item.signature,
                doc_summary: item.doc_summary,
            });
        }
        for method in extract.methods {
            rows.push(Row {
                path: file.path.clone(),
                head: format!("method {}::{}", method.owner, method.name),
                line: method.start_line,
                end_line: method.end_line,
                signature: method.signature,
                doc_summary: method.doc_summary,
            });
        }
    }
    rows
}

#[test]
fn every_item_and_method_matches_expected_exactly() {
    let want = expected_rows(EXPECTED);
    let got = extracted_rows();
    for (want, got) in want.iter().zip(&got) {
        assert_eq!(want, got);
    }
    assert_eq!(want.len(), got.len(), "{got:#?}");
}

#[test]
fn the_expected_rows_cover_the_spec_cases() {
    let rows = expected_rows(EXPECTED);
    assert!(rows.len() >= 12, "{} rows", rows.len());
    let row = |head: &str| {
        rows.iter()
            .find(|row| row.head == head)
            .unwrap_or_else(|| panic!("no row {head}"))
    };
    assert!(row("function largest").signature.contains(" where "));
    assert_eq!(row("struct Meters").signature, "pub struct Meters");
    assert_eq!(row("struct Stop").signature, "pub struct Stop");
    assert!(row("enum Heading").end_line > row("enum Heading").line);
    assert!(row("trait Area").doc_summary.is_some());
    assert!(row("method Grid::filled").doc_summary.is_some());
    assert!(row("method Meters::area").doc_summary.is_some());
    let cut = &row("function configure_everything").signature;
    assert_eq!(cut.chars().count(), 257);
    assert!(cut.ends_with('…'));
    let fits = &row("function fits_the_cap").signature;
    assert_eq!(fits.chars().count(), 256);
    assert!(!fits.ends_with('…'));
    let summary = row("function summarize").doc_summary.clone().expect("doc");
    assert_eq!(summary.chars().count(), 161);
    assert!(summary.ends_with("é…"));
    assert!(
        !row("struct Stop")
            .doc_summary
            .clone()
            .expect("doc")
            .ends_with('.')
    );
    for none in ["module long", "function zero", "function quiet"] {
        assert_eq!(row(none).doc_summary, None, "{none}");
    }
}
