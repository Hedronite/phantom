//! `fixtures/impact` against its `EXPECTED.md`, at the default options
//! (SPEC-w5 G2d).

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{Hit, Impact, ImpactOptions, impact};
use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/impact");
const EXPECTED: &str = include_str!("../../../fixtures/impact/EXPECTED.md");

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("phantom-fractal-analyze-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One `## Target NAME` section: its name and its `- key: value` lines.
struct Section {
    name: String,
    lines: Vec<(String, String)>,
}

fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(name) = heading.strip_prefix("Target ") {
                out.push(Section {
                    name: name.trim().to_owned(),
                    lines: Vec::new(),
                });
            } else {
                // Any other heading ends the current target.
                out.push(Section {
                    name: String::new(),
                    lines: Vec::new(),
                });
            }
            continue;
        }
        let (Some(section), Some(entry)) = (out.last_mut(), line.strip_prefix("- ")) else {
            continue;
        };
        if !section.name.is_empty()
            && let Some((key, value)) = entry.split_once(": ")
        {
            section.lines.push((key.to_owned(), value.to_owned()));
        }
    }
    out.retain(|section| !section.name.is_empty());
    out
}

/// `DEPTH PATH:LINE NAME CONFIDENCE REASON SITE_PATH:SITE_LINE`.
fn hit_line(hit: &Hit) -> String {
    let line = |line: Option<i64>| line.map_or_else(|| "?".to_owned(), |line| line.to_string());
    format!(
        "{} {}:{} {} {} {} {}:{}",
        hit.depth,
        hit.symbol.file,
        line(hit.symbol.line),
        hit.symbol.name,
        hit.confidence,
        hit.via.reason,
        hit.via.site_file,
        line(hit.via.site_line),
    )
}

/// The section's lines that `impact` should give, in `EXPECTED.md` form.
fn rendered(got: &Impact) -> Vec<(String, String)> {
    let reasons = if got.reasons.is_empty() {
        "none".to_owned()
    } else {
        got.reasons
            .iter()
            .map(|r| match r.count {
                Some(count) => format!("{}={count}", r.reason.as_str()),
                None => r.reason.as_str().to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let c = &got.counts;
    let mut out = vec![
        ("risk".to_owned(), got.risk.as_str().to_owned()),
        ("exact".to_owned(), got.exact.to_string()),
        ("reasons".to_owned(), reasons),
        (
            "counts".to_owned(),
            format!(
                "direct {}, total {}, packages {}, tests {}, below_floor {}",
                c.direct, c.total, c.packages, c.tests, c.below_floor
            ),
        ),
    ];
    for group in &got.groups {
        assert_eq!(group.total, group.hits.len());
        assert!(!group.truncated);
        out.extend(group.hits.iter().map(|h| ("hit".to_owned(), hit_line(h))));
    }
    out.extend(got.tests.iter().map(|h| ("test".to_owned(), hit_line(h))));
    out
}

/// Equal, with the confidence field of a hit line compared to 1e-9.
fn same(want: &(String, String), got: &(String, String)) -> bool {
    if want.0 != got.0 {
        return false;
    }
    if want.0 != "hit" && want.0 != "test" {
        return want.1 == got.1;
    }
    let want: Vec<&str> = want.1.split(' ').collect();
    let got: Vec<&str> = got.1.split(' ').collect();
    want.len() == 6
        && got.len() == 6
        && want.iter().zip(&got).enumerate().all(|(i, (w, g))| {
            if i != 3 {
                return w == g;
            }
            match (w.parse::<f64>(), g.parse::<f64>()) {
                (Ok(w), Ok(g)) => (w - g).abs() < 1e-9,
                _ => false,
            }
        })
}

fn indexed() -> (TempDir, Store) {
    let dir = TempDir::new();
    let mut store = Store::open(&dir.0.join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    let report = index(Path::new(FIXTURE), &mut store, &rust).expect("index");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    (dir, store)
}

#[test]
fn every_named_target_matches_expected_exactly() {
    let (_dir, store) = indexed();
    let targets = sections(EXPECTED);
    assert!(targets.len() >= 6, "{} targets", targets.len());
    for section in &targets {
        let found = store.symbols_matching(&section.name).expect("lookup");
        assert_eq!(found.len(), 1, "{}: {found:?}", section.name);
        let got = impact(&store, found[0].id, &ImpactOptions::default()).expect("impact");
        let got = rendered(&got);
        let matches = got.len() == section.lines.len()
            && section.lines.iter().zip(&got).all(|(w, g)| same(w, g));
        assert!(
            matches,
            "{}\nwant {:#?}\ngot {:#?}",
            section.name, section.lines, got
        );
    }
}

#[test]
fn the_parser_reads_every_target_line() {
    let targets = sections(EXPECTED);
    let names: Vec<&str> = targets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "root_fn",
            "hidden",
            "used_in_macros",
            "ping",
            "area",
            "doubled",
            "report"
        ]
    );
    for section in &targets {
        let keys: Vec<&str> = section.lines.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys[..4],
            ["risk", "exact", "reasons", "counts"],
            "{}",
            section.name
        );
        assert!(
            keys[4..].iter().all(|k| *k == "hit" || *k == "test"),
            "{}",
            section.name
        );
    }
}
