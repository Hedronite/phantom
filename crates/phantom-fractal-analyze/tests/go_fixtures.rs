//! Verifies `fixtures/go-basic`, `fixtures/go-embed-iface`, and `fixtures/go-testpkg`
//! against their `EXPECTED.md` ground truth at default options.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{Hit, Impact, ImpactOptions, impact};
use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-go-fixture-{}-{nanos}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

fn format_target(answer: &Impact) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    lines.push(("risk".to_owned(), answer.risk.as_str().to_owned()));
    lines.push(("exact".to_owned(), answer.exact.to_string()));
    let reasons = if answer.reasons.is_empty() {
        "none".to_owned()
    } else {
        answer
            .reasons
            .iter()
            .map(|r| match r.count {
                Some(n) => format!("{}={n}", r.reason.as_str()),
                None => r.reason.as_str().to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    lines.push(("reasons".to_owned(), reasons));
    let c = &answer.counts;
    lines.push((
        "counts".to_owned(),
        format!(
            "direct {}, total {}, packages {}, tests {}, below_floor {}",
            c.direct, c.total, c.packages, c.tests, c.below_floor
        ),
    ));
    for group in &answer.groups {
        for hit in &group.hits {
            lines.push(("hit".to_owned(), hit_line(hit)));
        }
    }
    for hit in &answer.tests {
        lines.push(("test".to_owned(), hit_line(hit)));
    }
    lines
}

fn index_fixture(name: &str) -> (TempDir, Store) {
    let tmp = TempDir::new();
    let db = tmp.0.join("graph.db");
    let mut store = Store::open(&db).expect("open");
    let lang = RustLanguage::new().expect("rust lang");
    let fixture = Path::new(FIXTURES_DIR).join(name);
    index(&fixture, &mut store, &lang).expect("index");
    (tmp, store)
}

fn assert_fixture(name: &str) {
    let expected_path = Path::new(FIXTURES_DIR).join(name).join("EXPECTED.md");
    let expected = fs::read_to_string(&expected_path).expect("read EXPECTED.md");
    let (_tmp, store) = index_fixture(name);
    let secs = sections(&expected);
    assert!(!secs.is_empty(), "{name}: no sections");
    for sec in secs {
        let matches = store.symbols_matching(&sec.name).expect("match");
        assert_eq!(
            matches.len(),
            1,
            "{name}: target {} -> {matches:?}",
            sec.name
        );
        let ans = impact(&store, matches[0].id, &ImpactOptions::default()).expect("impact");
        let got = format_target(&ans);
        assert_eq!(got, sec.lines, "{name}: mismatch for {}", sec.name);
    }
}

#[test]
fn go_basic_embed_iface_and_testpkg_fixtures_match_expected() {
    assert_fixture("go-basic");
    assert_fixture("go-embed-iface");
    assert_fixture("go-testpkg");
}

#[test]
fn impact_source_defaults_and_store_impl_expose_target_and_build_excluded() {
    use phantom_fractal_analyze::ImpactSource;
    use phantom_fractal_store::{CallHit, StoreError, SymbolFacts};

    struct MinimalSource;
    impl ImpactSource for MinimalSource {
        fn callers(&self, _id: i64) -> Result<Vec<CallHit>, StoreError> {
            Ok(Vec::new())
        }
        fn facts(&self, _id: i64) -> Result<Option<SymbolFacts>, StoreError> {
            Ok(None)
        }
        fn unresolved_named(&self, _name: &str) -> Result<i64, StoreError> {
            Ok(0)
        }
        fn macro_args_named(&self, _name: &str) -> Result<i64, StoreError> {
            Ok(0)
        }
        fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
            Ok(Some(0))
        }
    }

    let minimal = MinimalSource;
    assert_eq!(minimal.target_name(1).unwrap(), None);
    assert_eq!(minimal.build_excluded_count().unwrap(), None);

    let (_tmp, store) = index_fixture("go-testpkg");
    assert_eq!(ImpactSource::build_excluded_count(&store).unwrap(), Some(2));
    let add = store.symbols_matching("Add").unwrap();
    assert_eq!(add.len(), 1);
    assert_eq!(
        ImpactSource::target_name(&store, add[0].id)
            .unwrap()
            .as_deref(),
        Some("calc")
    );
    assert_eq!(ImpactSource::target_name(&store, 999_999).unwrap(), None);
}
