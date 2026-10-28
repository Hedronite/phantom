//! The walk on constructed graphs (SPEC-w5 G2b, G2d).

mod support;

use phantom_fractal_analyze::{Group, ImpactOptions, Risk};
use support::{Graph, hits, names, options, star};

#[test]
fn a_symbol_keeps_the_depth_it_is_first_reached_at() {
    // b calls both the target and a, so it is depth 1, not 2.
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(2, "a");
    g.function(3, "b");
    g.function(4, "c");
    g.call(2, 1, 0.9);
    g.call(3, 2, 1.0);
    g.call(3, 1, 0.5);
    g.call(4, 3, 1.0);
    let got = g.impact(1);
    assert_eq!(
        hits(&got),
        [
            (1, "a".to_owned(), 0.9),
            (1, "b".to_owned(), 0.5),
            (2, "c".to_owned(), 0.5),
        ]
    );
    let depths: Vec<u8> = got.groups.iter().map(|group| group.depth).collect();
    assert_eq!(depths, [1, 2]);
}

#[test]
fn confidence_is_the_best_product_from_the_previous_level_with_its_via() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(2, "weak");
    g.function(3, "strong");
    g.function(4, "both");
    g.call(2, 1, 0.5);
    g.call(3, 1, 0.9);
    g.call_with(4, 2, 1.0, "use-alias");
    g.call_with(4, 3, 0.95, "crate-path");
    let got = g.impact(1);
    let both = &got.groups[1].hits[0];
    assert_eq!(both.symbol.name, "both");
    assert!(
        (both.confidence - 0.855).abs() < 1e-12,
        "{}",
        both.confidence
    );
    assert_eq!(both.via.callee, 3);
    assert_eq!(both.via.reason, "crate-path");
    assert_eq!(both.via.site_file, "src/lib.rs");
    assert_eq!(both.via.site_line, Some(4003));
}

#[test]
fn a_tie_keeps_the_edge_from_the_first_symbol_in_hit_order() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(30, "late");
    g.function(20, "early");
    g.function(4, "top");
    g.call(30, 1, 0.9);
    g.call(20, 1, 0.9);
    // Met from `late` (line 30) after `early` (line 20) in hit order.
    g.call_with(4, 30, 1.0, "late-edge");
    g.call_with(4, 20, 1.0, "early-edge");
    let top = &g.impact(1).groups[1].hits[0];
    assert_eq!(top.via.callee, 20);
    assert_eq!(top.via.reason, "early-edge");
}

#[test]
fn a_cycle_and_the_target_never_reappear() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(2, "a");
    g.function(3, "b");
    g.call(2, 1, 1.0);
    g.call(3, 2, 1.0);
    g.call(2, 3, 1.0);
    // The target calls into its own callers.
    g.call(1, 2, 1.0);
    g.call(1, 3, 1.0);
    let got = g.impact(1);
    assert_eq!(names(&got), [(1, "a"), (2, "b")]);
    assert_eq!(got.counts.total, 2);
}

#[test]
fn a_target_that_calls_itself_has_no_hit() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.call(1, 1, 1.0);
    let got = g.impact(1);
    assert_eq!(got.groups, Vec::<Group>::new());
    assert_eq!(got.counts.total, 0);
    // A self-call one level out is ignored too.
    g.function(2, "a");
    g.call(2, 1, 1.0);
    g.call(2, 2, 1.0);
    assert_eq!(names(&g.impact(1)), [(1, "a")]);
}

#[test]
fn the_floor_keeps_exactly_min_confidence_and_drops_just_under() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(2, "at");
    g.function(3, "under");
    g.function(4, "beyond_at");
    g.function(5, "beyond_under");
    g.call(2, 1, 0.5);
    g.call(3, 1, 0.499_999_999);
    g.call(4, 2, 1.0);
    g.call(5, 3, 1.0);
    let got = g.impact_with(1, &options(3, 0.5, true));
    // `under` is dropped and not expanded, so `beyond_under` is unseen.
    assert_eq!(names(&got), [(1, "at"), (2, "beyond_at")]);
    assert_eq!(got.counts.below_floor, 1);
    // The product counts, not the edge: 0.5 * 0.5 is exactly the floor.
    let mut g = Graph::new();
    g.function(1, "t");
    g.function(2, "a");
    g.function(3, "b");
    g.call(2, 1, 0.5);
    g.call(3, 2, 0.5);
    assert_eq!(names(&g.impact_with(1, &options(3, 0.25, true))).len(), 2);
    let under = g.impact_with(1, &options(3, 0.250_000_001, true));
    assert_eq!(names(&under), [(1, "a")]);
    assert_eq!(under.counts.below_floor, 1);
}

#[test]
fn tests_are_listed_apart_and_never_expanded() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.test(2, "a_test");
    g.function(3, "calls_the_test");
    g.function(4, "plain");
    g.test(5, "deep_test");
    g.call(2, 1, 0.95);
    g.call(3, 2, 1.0);
    g.call(4, 1, 1.0);
    g.call(5, 4, 0.9);
    let got = g.impact(1);
    assert_eq!(names(&got), [(1, "plain")]);
    let tests: Vec<(u8, &str, f64)> = got
        .tests
        .iter()
        .map(|hit| (hit.depth, hit.symbol.name.as_str(), hit.confidence))
        .collect();
    assert_eq!(tests, [(1, "a_test", 0.95), (2, "deep_test", 0.9)]);
    assert_eq!(got.tests[0].via.callee, 1);
    assert_eq!(got.counts.tests, 2);
    assert_eq!(got.counts.direct, 1);
    assert_eq!(got.counts.total, 1);
}

#[test]
fn without_tests_a_test_is_left_out_and_still_not_expanded() {
    let mut g = Graph::new();
    g.function(1, "t");
    g.test(2, "a_test");
    g.function(3, "calls_the_test");
    g.test(4, "faint_test");
    g.call(2, 1, 1.0);
    g.call(3, 2, 1.0);
    g.call(4, 1, 0.1);
    let got = g.impact_with(1, &options(3, 0.2, false));
    assert_eq!(names(&got), Vec::<(u8, &str)>::new());
    assert!(got.tests.is_empty());
    assert_eq!(got.counts.tests, 0);
    // A test left out is not counted below the floor either.
    assert_eq!(got.counts.below_floor, 0);
    // With tests on, the faint one is below the floor.
    let on = g.impact_with(1, &options(3, 0.2, true));
    assert_eq!(on.counts.below_floor, 1);
    assert_eq!(on.counts.tests, 1);
}

/// A chain `t <- c1 <- c2 <- … <- c6`.
fn chain() -> Graph {
    let mut g = Graph::new();
    g.function(1, "t");
    for n in 1..=6 {
        g.function(n + 1, &format!("c{n}"));
        g.call(n + 1, n, 1.0);
    }
    g
}

#[test]
fn no_level_past_max_depth() {
    let g = chain();
    let depth = |max| {
        g.impact_with(1, &options(max, 0.2, true))
            .groups
            .iter()
            .map(|group| group.depth)
            .collect::<Vec<u8>>()
    };
    assert_eq!(depth(1), [1]);
    assert_eq!(depth(3), [1, 2, 3]);
    assert_eq!(depth(ImpactOptions::MAX_DEPTH), [1, 2, 3, 4, 5]);
    assert_eq!(g.impact(1).counts.total, 3);
}

#[test]
fn hits_sort_by_file_then_line_then_name() {
    let mut g = Graph::new();
    g.function(1, "t");
    for (id, name, file, line) in [
        (2, "z", "b.rs", Some(1)),
        (3, "b", "a.rs", Some(9)),
        (4, "a", "a.rs", Some(9)),
        (5, "y", "a.rs", Some(2)),
        (6, "x", "a.rs", None),
    ] {
        g.function(id, name);
        g.facts_mut(id).file = file.to_owned();
        g.facts_mut(id).line = line;
        g.call(id, 1, 1.0);
    }
    assert_eq!(
        names(&g.impact(1)),
        [(1, "y"), (1, "a"), (1, "b"), (1, "x"), (1, "z")]
    );
}

#[test]
fn a_group_keeps_100_hits_and_its_total() {
    let full = star(100, 0).impact(1);
    assert_eq!(full.groups[0].total, 100);
    assert_eq!(full.groups[0].hits.len(), 100);
    assert!(!full.groups[0].truncated);
    let over = star(101, 0).impact(1);
    assert_eq!(over.groups[0].total, 101);
    assert_eq!(over.groups[0].hits.len(), Group::MAX_HITS);
    assert!(over.groups[0].truncated);
    assert_eq!(over.groups[0].hits[99].symbol.name, "d099");
    assert_eq!(over.counts.direct, 101);
    // All 101 are expanded, not only the 100 kept.
    let mut g = star(101, 0);
    g.function(5000, "behind_the_last");
    g.call(5000, 200, 1.0);
    assert_eq!(g.impact(1).groups[1].hits[0].symbol.name, "behind_the_last");
}

#[test]
fn packages_count_distinct_named_packages_of_non_test_hits() {
    let mut g = Graph::new();
    g.function(1, "t");
    for (id, package) in [(2, Some("a")), (3, Some("a")), (4, Some("b")), (5, None)] {
        g.function(id, "f");
        g.facts_mut(id).package = package.map(str::to_owned);
        g.call(id, 1, 1.0);
    }
    g.test(6, "elsewhere");
    g.facts_mut(6).package = Some("c".to_owned());
    g.call(6, 1, 1.0);
    let got = g.impact(1);
    assert_eq!(got.counts.packages, 2);
    assert_eq!(got.counts.direct, 4);
    assert_eq!(got.counts.tests, 1);
}

#[test]
fn a_target_with_no_row_gives_an_unknown_inexact_answer() {
    let mut g = Graph::new();
    g.function(2, "a");
    g.call(2, 1, 1.0);
    let got = g.impact(1);
    assert_eq!(got.target, None);
    assert!(got.groups.is_empty());
    assert!(!got.exact);
    assert_eq!(got.risk, Risk::Unknown);
}

#[test]
fn options_check_their_ranges() {
    assert!(ImpactOptions::default().in_range());
    assert_eq!(ImpactOptions::default(), options(3, 0.2, true));
    for (depth, confidence, ok) in [
        (1, 1.0, true),
        (5, f64::MIN_POSITIVE, true),
        (0, 0.2, false),
        (6, 0.2, false),
        (3, 0.0, false),
        (3, -0.1, false),
        (3, 1.000_001, false),
        (3, f64::NAN, false),
    ] {
        assert_eq!(
            options(depth, confidence, true).in_range(),
            ok,
            "{depth} {confidence}"
        );
    }
}
