//! The scorer run (W10 G1, D3), ignored by default: it reads the
//! `PHANTOM_SCIP` export and the `PHANTOM_SCORE_ROOT` the PM names,
//! indexes that root into a temporary data dir with the crate's own
//! `index`, scores the two against each other, and prints one
//! deterministic, path-free JSON object. Two runs print the same
//! bytes; the baseline evidence records them.

#[path = "scip/mod.rs"]
mod scip;

use std::path::PathBuf;

use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

/// One throwaway data dir, removed with the run.
struct Temp {
    root: PathBuf,
}

impl Temp {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "phantom-fractal-score-{label}-{}-{nanos}",
            std::process::id()
        ));
        Self { root }
    }

    fn db(&self) -> PathBuf {
        self.root.join("repos").join("score").join("graph.db")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The baseline helper: one blake3 of the export, for
/// `evidence/w10/G1-backend.md`. It does what the evidence needs and
/// never runs in CI.
#[test]
#[ignore = "manual: blake3 of PHANTOM_SCIP for the baseline note"]
fn hash() {
    let scip =
        std::env::var("PHANTOM_SCIP").expect("PHANTOM_SCIP: the rust-analyzer export's path");
    let bytes = std::fs::read(&scip).unwrap_or_else(|err| panic!("{}: {err}", scip));
    println!("{} {}", blake3::hash(&bytes).to_hex(), scip);
}

#[test]
#[ignore = "manual: reads PHANTOM_SCIP and PHANTOM_SCORE_ROOT (GOAL-w10 D3)"]
fn score() {
    let scip =
        std::env::var("PHANTOM_SCIP").expect("PHANTOM_SCIP: the rust-analyzer export's path");
    let root = std::env::var("PHANTOM_SCORE_ROOT")
        .expect("PHANTOM_SCORE_ROOT: the tree the export carries");
    let bytes = std::fs::read(&scip).unwrap_or_else(|err| panic!("{}: {err}", scip));
    let parsed = scip::wire::parse_index(&bytes).unwrap_or_else(|err| panic!("{}: {err}", scip));

    let temp = Temp::new("w10");
    let mut store = Store::open(&temp.db()).expect("open score store");
    let root_path = PathBuf::from(&root);
    index(
        &root_path,
        &mut store,
        &RustLanguage::new().expect("grammar"),
    )
    .expect("index the scored root");
    let report = scip::score::score(&parsed, &store).expect("score");

    // Deterministic: two runs over the same inputs must print the same
    // bytes, and nothing in the reply may be a path.
    let again = scip::score::score(&parsed, &store).expect("score again");
    assert_eq!(report, again, "two runs of the same tree differ");
    let no_path = report
        .lines()
        .any(|line| line.contains("/Users/") || line.contains("/home/"));
    assert!(!no_path, "the report carries an absolute path: {report}");
    println!("{report}");
}

/// F1 per band: a hand-built export over an indexed two-file package.
/// The export carries `src/lib.rs` only, so the `local` edge
/// `caller -> one` is covered and corroborated, and the `path-exact`
/// edge `two -> one` from `src/out.rs` is neither. Each band holds both
/// edges, and its covered figure holds the one.
#[test]
fn each_band_carries_its_covered_figure() {
    let temp = Temp::new("bands");
    let repo = temp.root.join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("create src");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"bands\"\n").expect("manifest");
    std::fs::write(
        repo.join("src/lib.rs"),
        "mod out;\npub fn one() {}\npub fn caller() {\n    one();\n}\n",
    )
    .expect("lib");
    std::fs::write(
        repo.join("src/out.rs"),
        "pub fn two() {\n    crate::one();\n}\n",
    )
    .expect("out");
    let mut store = Store::open(&temp.db()).expect("open store");
    index(&repo, &mut store, &RustLanguage::new().expect("grammar")).expect("index");

    let occurrence = |line, first, last, name: &str, is_definition| scip::wire::Occurrence {
        range: scip::wire::Range {
            start_line: line,
            start_character: first,
            end_line: line,
            end_character: last,
        },
        symbol: format!("rust-analyzer cargo bands 0.0.0 crate#{name}()."),
        is_definition,
    };
    let export = scip::wire::ScipIndex {
        documents: vec![scip::wire::Document {
            relative_path: "src/lib.rs".to_owned(),
            occurrences: vec![
                occurrence(1, 7, 10, "one", true),
                occurrence(2, 7, 13, "caller", true),
                occurrence(3, 4, 7, "one", false),
            ],
        }],
    };
    let report = scip::score::score(&export, &store).expect("score");

    let band = "{\"edges\":2,\"corroborated\":1,\"precision\":0.5000,\
                \"covered\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000}}";
    let bands = format!("\"confidence\":{{\"0.5\":{band},\"0.85\":{band},\"all\":{band}}}");
    assert!(report.contains(&bands), "{report}");
    assert!(
        report.contains(
            "\"local\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000,\
             \"covered\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000}}"
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "\"path-exact\":{\"edges\":1,\"corroborated\":0,\"precision\":0.0000,\
             \"covered\":{\"edges\":0,\"corroborated\":0,\"precision\":0.0000}}"
        ),
        "{report}"
    );
}
