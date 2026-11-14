//! Helpers of the scorer run in `scip_score.rs`, moved here whole to keep
//! that file within 1000 lines: its throwaway data dir, and the SCIP
//! occurrence and line finders its hand-built cases share.

use std::path::PathBuf;

use crate::scip;

/// One throwaway data dir, removed with the run.
pub(crate) struct Temp {
    pub(crate) root: PathBuf,
}

impl Temp {
    pub(crate) fn new(label: &str) -> Self {
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

    pub(crate) fn db(&self) -> PathBuf {
        self.root.join("repos").join("score").join("graph.db")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One SCIP occurrence on a `crate#<name>()` symbol of one test
/// package, as the bands test builds them.
pub(crate) fn occurrence(
    package: &str,
    line: i64,
    first: i64,
    last: i64,
    name: &str,
    is_definition: bool,
) -> scip::wire::Occurrence {
    scip::wire::Occurrence {
        range: scip::wire::Range {
            start_line: line,
            start_character: first,
            end_line: line,
            end_character: last,
        },
        symbol: format!("rust-analyzer cargo {package} 0.0.0 crate#{name}()."),
        is_definition,
    }
}

/// The 0-based line of the `skip`th line that carries `needle`.
pub(crate) fn line_of(lines: &[&str], needle: &str, skip: usize) -> i64 {
    let mut found = 0;
    for (index, line) in lines.iter().enumerate() {
        if line.contains(needle) {
            if found == skip {
                return index as i64;
            }
            found += 1;
        }
    }
    panic!("needle not found: {needle}");
}
