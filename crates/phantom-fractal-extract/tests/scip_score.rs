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
