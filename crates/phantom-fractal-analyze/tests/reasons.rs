//! Completeness reasons, `exact`, and risk on constructed graphs
//! (SPEC-w5 G2c, G2d).

mod support;

use phantom_fractal_analyze::{CompletenessReason, ReasonCount, Risk};
use support::{Graph, star};

use CompletenessReason::{
    ExternalUsersPossible, MacroBoundary, ParseSkippedFiles, TraitDispatch, UnresolvedRefs,
};

fn target() -> Graph {
    let mut g = Graph::new();
    g.function(1, "t");
    g
}

fn reasons(g: &Graph) -> Vec<(CompletenessReason, Option<u64>)> {
    let got = g.impact(1);
    assert_eq!(got.exact, got.reasons.is_empty());
    got.reasons
        .iter()
        .map(|ReasonCount { reason, count }| (*reason, *count))
        .collect()
}

#[test]
fn a_private_function_with_nothing_unseen_is_exact() {
    let g = target();
    assert_eq!(reasons(&g), []);
    assert!(g.impact(1).exact);
}

#[test]
fn unresolved_refs_and_macro_boundary_count_refs_named_like_the_target() {
    let mut g = target();
    g.unresolved.insert("t".to_owned(), 0);
    g.macro_args.insert("t".to_owned(), 0);
    g.unresolved.insert("other".to_owned(), 5);
    assert_eq!(reasons(&g), []);
    g.unresolved.insert("t".to_owned(), 1);
    assert_eq!(reasons(&g), [(UnresolvedRefs, Some(1))]);
    g.unresolved.insert("t".to_owned(), 0);
    g.macro_args.insert("t".to_owned(), 3);
    assert_eq!(reasons(&g), [(MacroBoundary, Some(3))]);
}

#[test]
fn trait_dispatch_needs_a_method_with_a_trait() {
    let mut g = target();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    assert_eq!(reasons(&g), [], "a function with a trait name");
    g.facts_mut(1).kind = "method".to_owned();
    assert_eq!(reasons(&g), [(TraitDispatch, None)]);
    g.facts_mut(1).trait_name = None;
    assert_eq!(reasons(&g), [], "an inherent method");
}

#[test]
fn parse_skipped_files_applies_to_a_skip_or_an_unknown_count() {
    let mut g = target();
    g.rs_skipped = Some(0);
    assert_eq!(reasons(&g), []);
    g.rs_skipped = Some(2);
    assert_eq!(reasons(&g), [(ParseSkippedFiles, Some(2))]);
    g.rs_skipped = None;
    assert_eq!(reasons(&g), [(ParseSkippedFiles, None)]);
}

#[test]
fn external_users_possible_needs_pub_in_a_lib() {
    for (visibility, target_kind, applies) in [
        ("public", Some("lib"), true),
        ("public", Some("bin"), false),
        ("public", None, false),
        ("crate", Some("lib"), false),
        ("restricted:pub(super)", Some("lib"), false),
        ("private", Some("lib"), false),
    ] {
        let mut g = target();
        g.facts_mut(1).visibility = visibility.to_owned();
        g.facts_mut(1).target_kind = target_kind.map(str::to_owned);
        let want: &[(CompletenessReason, Option<u64>)] = if applies {
            &[(ExternalUsersPossible, None)]
        } else {
            &[]
        };
        assert_eq!(reasons(&g), want, "{visibility} {target_kind:?}");
    }
}

#[test]
fn every_reason_at_once_keeps_the_fixed_order() {
    let mut g = target();
    g.facts_mut(1).kind = "method".to_owned();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    g.facts_mut(1).visibility = "public".to_owned();
    g.unresolved.insert("t".to_owned(), 4);
    g.macro_args.insert("t".to_owned(), 2);
    g.rs_skipped = Some(7);
    assert_eq!(
        reasons(&g),
        [
            (UnresolvedRefs, Some(4)),
            (MacroBoundary, Some(2)),
            (TraitDispatch, None),
            (ParseSkippedFiles, Some(7)),
            (ExternalUsersPossible, None),
        ]
    );
    let names: Vec<&str> = CompletenessReason::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(
        names,
        [
            "unresolved_refs",
            "macro_boundary",
            "trait_dispatch",
            "parse_skipped_files",
            "external_users_possible",
        ]
    );
    assert!(!g.impact(1).exact);
}

fn risk(direct: i64, second: i64) -> Risk {
    star(direct, second).impact(1).risk
}

/// `star(direct, 0)` with `unseen` unresolved refs named like the target,
/// split between unresolved and macro refs.
fn risk_unseen(direct: i64, unseen: i64) -> Risk {
    let mut g = star(direct, 0);
    g.unresolved.insert("t".to_owned(), unseen / 2);
    g.macro_args.insert("t".to_owned(), unseen - unseen / 2);
    g.impact(1).risk
}

#[test]
fn direct_boundaries_give_medium_high_and_critical() {
    assert_eq!(risk(0, 0), Risk::Low);
    assert_eq!(risk(2, 0), Risk::Low);
    assert_eq!(risk(3, 0), Risk::Medium);
    assert_eq!(risk(9, 0), Risk::Medium);
    assert_eq!(risk(10, 0), Risk::High);
    assert_eq!(risk(24, 0), Risk::High);
    assert_eq!(risk(25, 0), Risk::Critical);
}

#[test]
fn total_boundaries_give_medium_and_high() {
    // One direct caller plus `second` at depth 2.
    assert_eq!(star(1, 8).impact(1).counts.total, 9);
    assert_eq!(risk(1, 8), Risk::Low);
    assert_eq!(risk(1, 9), Risk::Medium);
    assert_eq!(risk(1, 48), Risk::Medium);
    assert_eq!(risk(1, 49), Risk::High);
}

#[test]
fn unknown_starts_where_unseen_reaches_max_one_direct() {
    assert_eq!(risk_unseen(4, 4), Risk::Unknown);
    assert_eq!(risk_unseen(4, 3), Risk::Medium);
    assert_eq!(risk_unseen(2, 2), Risk::Unknown);
    assert_eq!(risk_unseen(2, 1), Risk::Low);
    assert_eq!(risk_unseen(0, 1), Risk::Unknown);
    assert_eq!(risk_unseen(1, 1), Risk::Unknown);
    assert_eq!(risk_unseen(0, 0), Risk::Low);
    // Critical is checked first.
    assert_eq!(risk_unseen(25, 100), Risk::Critical);
    assert_eq!(risk_unseen(24, 24), Risk::Unknown);
    assert_eq!(risk_unseen(24, 23), Risk::High);
}

#[test]
fn only_unresolved_and_macro_counts_are_unseen() {
    // Null-count reasons and a skipped-file count never make it unknown.
    let mut g = star(1, 0);
    g.facts_mut(1).kind = "method".to_owned();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    g.facts_mut(1).visibility = "public".to_owned();
    g.rs_skipped = Some(50);
    let got = g.impact(1);
    assert_eq!(got.reasons.len(), 3);
    assert_eq!(got.risk, Risk::Low);
}

#[test]
fn tests_never_count_toward_risk() {
    let mut g = star(2, 0);
    for n in 0..30 {
        g.test(500 + n, &format!("test{n}"));
        g.call(500 + n, 1, 1.0);
    }
    let got = g.impact(1);
    assert_eq!(got.counts.tests, 30);
    assert_eq!(got.risk, Risk::Low);
    assert_eq!(Risk::Unknown.as_str(), "unknown");
}
