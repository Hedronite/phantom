//! `callers_distinct`, `files` and `sites`: the hits of `callers`,
//! `callees` and `impact` grouped by the symbol they name (SPEC-w14 G1b).
//!
//! The keys sit beside a reply's existing ones and are built from the
//! hits it lists, after its own cap. Under a budget they go first, as a
//! whole, before any hit (see [`super::budget::Shed::drop_extras`]).

use std::collections::BTreeMap;

use serde::Serialize;

/// One hit as the grouping reads it: the symbol's display name and file,
/// the call-site line, and the confidence.
pub(crate) struct Site<'a> {
    pub(crate) display: &'a str,
    pub(crate) file: &'a str,
    pub(crate) line: Option<i64>,
    pub(crate) confidence: f64,
}

/// The new keys of one reply.
#[derive(Clone, Serialize)]
pub(crate) struct Distinct {
    /// One entry per display name, sorted by it.
    callers_distinct: Vec<Group>,
    /// The files the listed symbols are declared in, distinct and sorted.
    files: Vec<String>,
    /// How many hits were listed.
    sites: usize,
}

/// The hits that name one symbol: its call-site lines, sorted, and the
/// lowest confidence among them.
#[derive(Clone, Serialize)]
struct Group {
    display: String,
    sites: Vec<Option<i64>>,
    min_confidence: f64,
}

impl Distinct {
    /// The keys for `sites`, one per listed hit.
    pub(crate) fn of<'a>(sites: impl IntoIterator<Item = Site<'a>>) -> Self {
        let mut groups: BTreeMap<&str, Group> = BTreeMap::new();
        let mut files: Vec<String> = Vec::new();
        let mut count = 0;
        for site in sites {
            count += 1;
            files.push(site.file.to_owned());
            let group = groups.entry(site.display).or_insert_with(|| Group {
                display: site.display.to_owned(),
                sites: Vec::new(),
                min_confidence: site.confidence,
            });
            group.sites.push(site.line);
            group.min_confidence = group.min_confidence.min(site.confidence);
        }
        files.sort_unstable();
        files.dedup();
        let callers_distinct = groups
            .into_values()
            .map(|mut group| {
                group.sites.sort_unstable();
                group
            })
            .collect();
        Self {
            callers_distinct,
            files,
            sites: count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Distinct, Site};

    fn site(
        display: &'static str,
        file: &'static str,
        line: i64,
        confidence: f64,
    ) -> Site<'static> {
        Site {
            display,
            file,
            line: Some(line),
            confidence,
        }
    }

    #[test]
    fn groups_by_display_with_sorted_lines_and_the_lowest_confidence() {
        let distinct = Distinct::of([
            site("src/b.rs::run", "src/b.rs", 9, 0.9),
            site("src/a.rs::App::start", "src/a.rs", 30, 0.8),
            site("src/b.rs::run", "src/b.rs", 4, 0.6),
            site("src/a.rs::App::start", "src/a.rs", 12, 0.95),
        ]);
        let json = serde_json::to_value(&distinct).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "callers_distinct": [
                    { "display": "src/a.rs::App::start", "sites": [12, 30], "min_confidence": 0.8 },
                    { "display": "src/b.rs::run", "sites": [4, 9], "min_confidence": 0.6 },
                ],
                "files": ["src/a.rs", "src/b.rs"],
                "sites": 4,
            })
        );
    }

    #[test]
    fn no_hits_give_empty_lists_and_zero_sites() {
        let json = serde_json::to_value(Distinct::of([])).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "callers_distinct": [], "files": [], "sites": 0 })
        );
    }

    #[test]
    fn a_site_without_a_line_sorts_first_as_null() {
        let distinct = Distinct::of([
            site("src/a.rs::f", "src/a.rs", 3, 1.0),
            Site {
                display: "src/a.rs::f",
                file: "src/a.rs",
                line: None,
                confidence: 1.0,
            },
        ]);
        let json = serde_json::to_value(&distinct).expect("json");
        assert_eq!(
            json["callers_distinct"][0]["sites"],
            serde_json::json!([null, 3])
        );
    }
}
