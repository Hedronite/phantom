//! The map of indexed fixtures against goldens, and its use of the
//! store (SPEC-w7 G2).
//!
//! Each golden is `tests/goldens/map/<fixture>-<detail>-<budget>.txt`.
//! `PHANTOM_BLESS=1` rewrites them from the current output.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{
    Detail, Map, MapError, MapRequest, estimate_tokens, packing_budget, rank, render,
};
use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures");
const GOLDENS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens/map");
const BUDGETS: [u64; 3] = [60, 150, 500];
const DETAILS: [(Detail, &str); 3] = [
    (Detail::Files, "files"),
    (Detail::Symbols, "symbols"),
    (Detail::Full, "full"),
];

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-analyze-map-{tag}-{}-{nanos}",
            process::id()
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

/// `fixtures/<name>` indexed into a fresh store. The fixture is only read.
fn indexed(name: &str) -> (TempDir, Store) {
    let db = TempDir::new(name);
    let mut store = Store::open(&db.0.join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");
    index(&Path::new(FIXTURES).join(name), &mut store, &rust).expect("index");
    (db, store)
}

fn db_bytes(db: &TempDir) -> Vec<Vec<u8>> {
    ["graph.db", "graph.db-wal"]
        .iter()
        .map(|name| match fs::read(db.0.join(name)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => Vec::new(),
            Err(err) => panic!("read {name}: {err}"),
        })
        .collect()
}

fn request(budget: u64, detail: Detail) -> MapRequest {
    MapRequest {
        budget,
        focus_files: Vec::new(),
        detail,
    }
}

/// `ceil(bytes / 3.5) + 1`, written out again here.
fn estimate(line: &str) -> u64 {
    let bytes = line.len() as f64;
    (bytes / 3.5).ceil() as u64 + 1
}

/// Asserts the map's own accounting: the text's estimate is `used_tokens`,
/// within the budget and stated in the header, and every line is ASCII.
fn assert_accounted(map: &Map, budget: u64) {
    let used: u64 = map.text.lines().map(estimate).sum();
    assert_eq!(map.used_tokens, used, "{}", map.text);
    assert_eq!(estimate_tokens(&map.text), used);
    assert!(used <= packing_budget(budget), "{used} over {budget}");
    assert!(map.text.is_ascii());
    assert!(map.text.ends_with('\n'));
    let header = map.text.lines().next().expect("header");
    assert_eq!(
        header,
        format!(
            "phantom-fractal map | budget {budget} | used {used} | rank {}",
            map.rank_epoch
        )
    );
    let footer = map.text.lines().last().expect("footer");
    assert_eq!(
        footer,
        format!(
            "-- omitted: {} files, {} symbols (raise --budget or use --focus)",
            map.files_omitted, map.symbols_omitted
        )
    );
}

/// Compares `text` with golden `name`, or writes it under `PHANTOM_BLESS`.
fn assert_golden(name: &str, text: &str) {
    let path = Path::new(GOLDENS).join(format!("{name}.txt"));
    if std::env::var_os("PHANTOM_BLESS").is_some() {
        fs::create_dir_all(GOLDENS).expect("golden dir");
        fs::write(&path, text).expect("write golden");
        return;
    }
    let want = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert_eq!(text, want, "golden {name}");
}

fn goldens(fixture: &str) {
    let (_db, store) = indexed(fixture);
    for budget in BUDGETS {
        for (detail, label) in DETAILS {
            let map = render(&store, &request(budget, detail)).expect("map");
            assert_accounted(&map, budget);
            assert_golden(&format!("{fixture}-{label}-{budget}"), &map.text);
        }
    }
}

#[test]
fn impact_goldens_at_three_budgets_and_every_detail() {
    goldens("impact");
}

#[test]
fn graph_goldens_at_three_budgets_and_every_detail() {
    goldens("graph");
}

#[test]
fn a_focus_puts_its_file_first_under_the_personalised_rank() {
    let (_db, store) = indexed("graph");
    let uses = store
        .files()
        .expect("files")
        .into_iter()
        .find(|file| file.path == "app/src/uses.rs")
        .expect("uses.rs")
        .id;
    let map = render(
        &store,
        &MapRequest {
            budget: 150,
            focus_files: vec![uses, 9_999],
            detail: Detail::Symbols,
        },
    )
    .expect("map");
    assert_accounted(&map, 150);
    assert_golden("graph-focus-uses-150", &map.text);
    assert!(
        map.text
            .lines()
            .nth(1)
            .expect("first file")
            .starts_with("app/src/uses.rs  [r ")
    );
}

#[test]
fn the_counts_cover_every_file_and_symbol() {
    let (_db, store) = indexed("impact");
    let files = store.files().expect("files").len() as u64;
    let symbols = store.map_rows().expect("rows").len() as u64;
    for budget in [40, 60, 150, 500, 100_000] {
        for (detail, _) in DETAILS {
            let map = render(&store, &request(budget, detail)).expect("map");
            assert_eq!(map.files_shown + map.files_omitted, files);
            assert_eq!(map.symbols_shown + map.symbols_omitted, symbols);
            let body: Vec<&str> = map.text.lines().skip(1).collect();
            let file_lines = body.iter().filter(|line| !line.starts_with(' ')).count() as u64;
            let symbol_lines = body
                .iter()
                .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
                .count() as u64;
            // The footer is the one unindented line that is not a file.
            assert_eq!(file_lines - 1, map.files_shown);
            assert_eq!(symbol_lines, map.symbols_shown);
            if detail == Detail::Files {
                assert_eq!(map.symbols_shown, 0);
            }
        }
    }
    // Everything fits in a large budget.
    let map = render(&store, &request(100_000, Detail::Full)).expect("map");
    assert_eq!((map.files_omitted, map.symbols_omitted), (0, 0));
}

#[test]
fn a_budget_below_header_and_footer_is_too_small() {
    let (_db, store) = indexed("graph");
    let files = store.files().expect("files").len();
    let symbols = store.map_rows().expect("rows").len();
    let footer =
        format!("-- omitted: {files} files, {symbols} symbols (raise --budget or use --focus)");
    // The header is reserved at its widest: `used` as large as the budget.
    let needed = |budget: u64| {
        estimate(&format!(
            "phantom-fractal map | budget {budget} | used {budget} | rank 1"
        )) + estimate(&footer)
    };
    let smallest = (1..1_000)
        .find(|budget| needed(*budget) <= packing_budget(*budget))
        .expect("some budget fits");
    for budget in 0..smallest {
        match render(&store, &request(budget, Detail::Symbols)) {
            Err(MapError::BudgetTooSmall {
                budget: got,
                needed: want,
            }) => {
                assert_eq!((got, want), (budget, needed(budget)));
            }
            other => panic!("budget {budget}: {other:?}"),
        }
    }
    let map = render(&store, &request(smallest, Detail::Symbols)).expect("map");
    assert_accounted(&map, smallest);
    assert_eq!(map.files_shown, 0);
    let err = render(&store, &request(10, Detail::Files)).expect_err("too small");
    assert!(
        err.to_string().starts_with("budget 10 is too small"),
        "{err}"
    );
}

#[test]
fn render_never_writes_and_repeats_byte_for_byte() {
    let (db, store) = indexed("graph");
    let before = db_bytes(&db);
    for (detail, _) in DETAILS {
        let first = render(&store, &request(500, detail)).expect("map");
        assert_eq!(render(&store, &request(500, detail)).expect("map"), first);
    }
    assert_eq!(db_bytes(&db), before);
}

#[test]
fn the_stored_rank_is_used_only_at_the_current_epoch() {
    let (_db, mut store) = indexed("graph");
    let epoch = store.graph_epoch().expect("epoch");
    let computed = render(&store, &request(500, Detail::Files)).expect("map");
    let files = store.files().expect("files");
    let lib = files
        .iter()
        .find(|file| file.path == "app/src/lib.rs")
        .expect("lib.rs")
        .id;
    // A stored rank that puts the computed last file first.
    let ranks = rank(&store).expect("rank");
    let stored_files: Vec<(i64, f64)> = files
        .iter()
        .map(|file| (file.id, if file.id == lib { 1.0 } else { 0.0 }))
        .collect();
    store
        .write_rank(epoch, &ranks.symbols, &stored_files)
        .expect("write rank");
    let stored = render(&store, &request(500, Detail::Files)).expect("map");
    assert_eq!(stored.rank_epoch, epoch);
    assert_eq!(
        stored.text.lines().nth(1).expect("first file"),
        "app/src/lib.rs  [r 1.00]"
    );
    // At another epoch the stored rank is stale, so rank is computed.
    store
        .write_rank(epoch + 1, &ranks.symbols, &stored_files)
        .expect("write rank");
    assert_eq!(
        render(&store, &request(500, Detail::Files)).expect("map"),
        computed
    );
}
