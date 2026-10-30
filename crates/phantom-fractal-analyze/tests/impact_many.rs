//! `impact_many` (SPEC-w9 G2a): one walk from every changed symbol at
//! once, on constructed graphs.

mod support;

use phantom_fractal_analyze::{
    CompletenessReason, DiffState, ImpactOptions, ManyImpact, Risk, impact_many,
};
use support::{Graph, star};

fn many(g: &Graph, sources: &[i64], diff: DiffState) -> ManyImpact {
    impact_many(g, sources, &ImpactOptions::default(), &diff).expect("impact_many")
}

/// `(depth, name, confidence, via callee)` for every reported hit.
fn hits(found: &ManyImpact) -> Vec<(u8, String, f64, i64)> {
    found
        .groups
        .iter()
        .flat_map(|group| {
            group.hits.iter().map(|hit| {
                (
                    hit.depth,
                    hit.symbol.name.clone(),
                    hit.confidence,
                    hit.via.callee,
                )
            })
        })
        .collect()
}

fn reasons(found: &ManyImpact) -> Vec<(&'static str, Option<u64>)> {
    found
        .reasons
        .iter()
        .map(|reason| (reason.reason.as_str(), reason.count))
        .collect()
}

#[test]
fn one_source_is_exactly_impact() {
    for g in [star(3, 2), star(12, 40), star(30, 0)] {
        let single = g.impact(1);
        let found = many(&g, &[1], DiffState::default());
        assert_eq!(found.sources, [single.target.clone().expect("target")]);
        assert_eq!(found.groups, single.groups);
        assert_eq!(found.tests, single.tests);
        assert_eq!(found.counts, single.counts);
        assert_eq!(found.reasons, single.reasons);
        assert_eq!(found.exact, single.exact);
        assert_eq!(found.risk, single.risk);
    }
}

#[test]
fn a_caller_of_two_sources_is_placed_once_at_its_nearest_depth_by_its_best_path() {
    let mut g = Graph::new();
    g.function(1, "s");
    g.function(2, "t");
    g.function(3, "mid");
    g.function(4, "both");
    g.function(5, "top");
    g.call(3, 1, 1.0);
    g.call(4, 3, 1.0); // depth 2 from s
    g.call(4, 2, 0.5); // depth 1 from t
    g.call(5, 4, 1.0);
    g.call(5, 2, 0.95);
    let found = many(&g, &[1, 2], DiffState::default());
    assert_eq!(
        hits(&found),
        [
            (1, "mid".to_owned(), 1.0, 1),
            (1, "both".to_owned(), 0.5, 2),
            (1, "top".to_owned(), 0.95, 2),
        ]
    );
    assert_eq!(found.counts.direct, 3);
    assert_eq!(found.counts.total, 3);
}

#[test]
fn a_source_is_never_a_hit_even_when_it_calls_another_source() {
    let mut g = Graph::new();
    g.function(1, "s");
    g.function(2, "t");
    g.function(3, "u");
    g.call(2, 1, 1.0);
    g.call(3, 2, 1.0);
    let found = many(&g, &[1, 2], DiffState::default());
    assert_eq!(hits(&found), [(1, "u".to_owned(), 1.0, 2)]);
    let names: Vec<&str> = found.sources.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["s", "t"]);
}

#[test]
fn the_answer_does_not_depend_on_the_order_or_repeats_of_the_sources() {
    let g = {
        // `d001` reaches `t` at 1.0 and `other` at 0.9; `s001`, a caller
        // of `d000`, also calls `other`.
        let mut g = star(5, 3);
        g.function(500, "other");
        g.call(101, 500, 0.9);
        g.call(10_001, 500, 0.4);
        g
    };
    let want = many(&g, &[1, 500], DiffState::default());
    assert_eq!(hits(&want)[1], (1, "d001".to_owned(), 1.0, 1));
    assert!(hits(&want).contains(&(1, "s001".to_owned(), 0.4, 500)));
    for sources in [vec![500, 1], vec![1, 500, 1], vec![500, 500, 1, 1]] {
        assert_eq!(
            many(&g, &sources, DiffState::default()),
            want,
            "{sources:?}"
        );
    }
}

#[test]
fn a_source_without_a_row_is_left_out() {
    let g = star(2, 0);
    let found = many(&g, &[1, 999], DiffState::default());
    assert_eq!(found.sources.len(), 1);
    assert_eq!(found, many(&g, &[1], DiffState::default()));
    let none = many(&g, &[999], DiffState::default());
    assert!(none.sources.is_empty() && none.groups.is_empty());
    assert_eq!(none.risk, Risk::Low);
    assert!(none.exact);
}

#[test]
fn reasons_are_summed_over_distinct_names_and_apply_when_any_source_has_them() {
    let mut g = Graph::new();
    g.function(1, "s");
    g.function(2, "t");
    g.function(3, "s");
    g.unresolved.insert("s".to_owned(), 2);
    g.unresolved.insert("t".to_owned(), 3);
    g.macro_args.insert("t".to_owned(), 1);
    g.facts_mut(2).kind = "method".to_owned();
    g.facts_mut(2).trait_name = Some("Area".to_owned());
    g.facts_mut(3).visibility = "public".to_owned();
    g.rs_skipped = Some(4);
    let found = many(&g, &[1, 2, 3], DiffState::default());
    assert_eq!(
        reasons(&found),
        [
            ("unresolved_refs", Some(5)),
            ("macro_boundary", Some(1)),
            ("trait_dispatch", None),
            ("parse_skipped_files", Some(4)),
            ("external_users_possible", None),
        ]
    );
    assert!(!found.exact);
    // Unresolved plus macro refs (6) reach max(1, direct = 0).
    assert_eq!(found.risk, Risk::Unknown);
}

#[test]
fn stale_index_and_unindexed_changes_come_last_and_only_when_they_apply() {
    let g = star(2, 0);
    let clean = many(&g, &[1], DiffState::default());
    assert!(clean.exact, "{:?}", clean.reasons);
    let stale = many(
        &g,
        &[1],
        DiffState {
            stale_index: true,
            unindexed_changes: 3,
        },
    );
    assert_eq!(
        reasons(&stale),
        [("stale_index", None), ("unindexed_changes", Some(3))]
    );
    assert!(!stale.exact);
    assert_eq!(stale.risk, clean.risk);
    assert_eq!(stale.groups, clean.groups);
    let only_unindexed = many(
        &g,
        &[],
        DiffState {
            stale_index: false,
            unindexed_changes: 1,
        },
    );
    assert_eq!(reasons(&only_unindexed), [("unindexed_changes", Some(1))]);
    let zero = many(
        &g,
        &[1],
        DiffState {
            stale_index: false,
            unindexed_changes: 0,
        },
    );
    assert!(zero.exact);
    assert_eq!(
        CompletenessReason::ALL[5..],
        [
            CompletenessReason::StaleIndex,
            CompletenessReason::UnindexedChanges
        ]
    );
}
