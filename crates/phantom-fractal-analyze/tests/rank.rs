//! PageRank on small graphs with known scores (SPEC-w7 G1b).
//!
//! Each expected list is what the iteration gives, to 1e-9: an independent
//! scratch script ran the same update and stop rule on the same weighted
//! edges. The iteration stops within 1e-6 in L1, not at the fixed point,
//! so each test also checks the fixed point solved by hand, to 1e-5.
//! A test symbol's edges carry [`TEST_FACTOR`] of its damped score.

use phantom_fractal_analyze::{
    EdgeKind, IMPL_WEIGHT, IMPORT_WEIGHT, Iteration, PRIOR_BOOST, RankEdge, RankNode, RankSource,
    Ranks, TEST_FACTOR, pagerank, personalised, rank,
};
use phantom_fractal_store::StoreError;

#[test]
fn the_weights_and_the_stop_rule_are_the_spec_constants() {
    assert_eq!(IMPL_WEIGHT, 0.7);
    assert_eq!(TEST_FACTOR, 0.2);
    assert_eq!(PRIOR_BOOST, 2.0);
    assert_eq!(IMPORT_WEIGHT, 0.3);
    assert_eq!(
        Iteration::SPEC,
        Iteration {
            damping: 0.85,
            tolerance: 1e-6,
            max_iterations: 100,
        }
    );
}

/// A hand-built graph.
#[derive(Default, Clone)]
struct Graph {
    nodes: Vec<RankNode>,
    edges: Vec<RankEdge>,
    imports: Vec<(i64, i64)>,
}

impl RankSource for Graph {
    fn graph_epoch(&self) -> Result<u64, StoreError> {
        Ok(7)
    }

    fn nodes(&self) -> Result<Vec<RankNode>, StoreError> {
        Ok(self.nodes.clone())
    }

    fn edges(&self) -> Result<Vec<RankEdge>, StoreError> {
        Ok(self.edges.clone())
    }

    fn imports(&self) -> Result<Vec<(i64, i64)>, StoreError> {
        Ok(self.imports.clone())
    }
}

impl Graph {
    /// Adds symbol `id` in file `file`.
    fn node(mut self, id: i64, file: i64) -> Self {
        self.nodes.push(RankNode {
            id,
            file_id: file,
            is_test: false,
            boosted: false,
        });
        self
    }

    fn test_node(mut self, id: i64, file: i64) -> Self {
        self = self.node(id, file);
        self.nodes.last_mut().expect("node").is_test = true;
        self
    }

    fn boosted_node(mut self, id: i64, file: i64) -> Self {
        self = self.node(id, file);
        self.nodes.last_mut().expect("node").boosted = true;
        self
    }

    fn call(mut self, source: i64, target: i64, confidence: f64) -> Self {
        self.edges.push(RankEdge {
            kind: EdgeKind::Call,
            source,
            target,
            confidence,
        });
        self
    }

    fn implements(mut self, source: i64, target: i64, confidence: f64) -> Self {
        self.edges.push(RankEdge {
            kind: EdgeKind::Impl,
            source,
            target,
            confidence,
        });
        self
    }

    fn import(mut self, from: i64, to: i64) -> Self {
        self.imports.push((from, to));
        self
    }
}

fn ranked(graph: &Graph) -> Ranks {
    let ranks = rank(graph).expect("rank");
    assert_sums_to_one(&ranks);
    ranks
}

fn assert_sums_to_one(ranks: &Ranks) {
    for list in [&ranks.symbols, &ranks.files] {
        if !list.is_empty() {
            let sum: f64 = list.iter().map(|(_, score)| score).sum();
            assert!((sum - 1.0).abs() < 1e-9, "sum {sum}");
        }
    }
}

/// Asserts `got` is `want`'s ids with scores within `tolerance`.
fn assert_scores(got: &[(i64, f64)], want: &[(i64, f64)], tolerance: f64) {
    assert_eq!(got.len(), want.len(), "{got:?}");
    for ((id, score), (want_id, want_score)) in got.iter().zip(want) {
        assert_eq!(id, want_id);
        assert!(
            (score - want_score).abs() < tolerance,
            "{id}: {score} against {want_score}"
        );
    }
}

/// `10 -> 20 -> 30`, one file each. The sink is dangling, so its score
/// returns along the uniform prior.
fn chain() -> Graph {
    Graph::default()
        .node(10, 1)
        .node(20, 2)
        .node(30, 3)
        .call(10, 20, 1.0)
        .call(20, 30, 1.0)
}

#[test]
fn a_chain_gathers_toward_its_dangling_end() {
    let ranks = ranked(&chain());
    let iterated = [
        (10, 0.18441687554671377),
        (20, 0.3411710064820743),
        (30, 0.4744121179712114),
    ];
    assert_scores(&ranks.symbols, &iterated, 1e-9);
    // With c = (0.15 + 0.85 r30) / 3: r10 = c, r20 = 1.85c, r30 = 2.5725c,
    // and the three sum to 5.4225c = 1.
    let c = 1.0 / 5.4225;
    assert_scores(
        &ranks.symbols,
        &[(10, c), (20, 1.85 * c), (30, 2.5725 * c)],
        1e-5,
    );
    // One symbol per file and one edge per file pair: the same graph.
    assert_scores(
        &ranks.files,
        &[(1, iterated[0].1), (2, iterated[1].1), (3, iterated[2].1)],
        1e-9,
    );
    assert_eq!(ranks.epoch, 7);
}

#[test]
fn a_star_puts_the_spokes_mass_on_its_hub() {
    // Three spokes call the hub. The confidence cancels in each one-edge row.
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 1)
        .node(3, 1)
        .node(4, 1)
        .call(2, 1, 0.9)
        .call(3, 1, 0.9)
        .call(4, 1, 0.9);
    let ranks = ranked(&graph);
    let spoke = 0.15267180962719834;
    assert_scores(
        &ranks.symbols,
        &[(1, 0.5419845711184047), (2, spoke), (3, spoke), (4, spoke)],
        1e-9,
    );
    // s = (0.15 + 0.85h) / 4 and h = 3.55s, so 6.55s = 1.
    let s = 1.0 / 6.55;
    assert_scores(
        &ranks.symbols,
        &[(1, 3.55 * s), (2, s), (3, s), (4, s)],
        1e-5,
    );
    // One file: no file edge, so its whole mass.
    assert_scores(&ranks.files, &[(1, 1.0)], 1e-12);
}

#[test]
fn a_cycle_under_a_uniform_prior_is_uniform_after_one_iteration() {
    let edges = [(0, 1, 1.0), (1, 2, 1.0), (2, 0, 1.0)];
    let run = pagerank(3, &edges, &[1.0; 3], &[1.0; 3], &Iteration::SPEC);
    assert_eq!(run.iterations, 1);
    for score in &run.scores {
        assert!((score - 1.0 / 3.0).abs() < 1e-15, "{score}");
    }
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 2)
        .node(3, 3)
        .call(1, 2, 0.95)
        .call(2, 3, 1.0)
        .call(3, 1, 0.9);
    let third = 1.0 / 3.0;
    assert_scores(
        &ranked(&graph).symbols,
        &[(1, third), (2, third), (3, third)],
        1e-15,
    );
}

#[test]
fn a_dangling_node_spreads_its_score_along_the_prior() {
    // 1 calls 2 at 1.0 and 3 at 0.5, so its row is 2/3 and 1/3; 3 calls
    // back to 1; 2 calls nothing.
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 1)
        .node(3, 1)
        .call(1, 2, 1.0)
        .call(1, 3, 0.5)
        .call(3, 1, 1.0);
    let ranks = ranked(&graph);
    assert_scores(
        &ranks.symbols,
        &[
            (1, 0.374430718882723),
            (2, 0.3658290197957532),
            (3, 0.2597402613215235),
        ],
        1e-9,
    );
    // With k = (0.15 + 0.85 r2) / 3: r1 = k + 0.85 r3, r2 = k + 0.85 (2/3) r1,
    // r3 = k + 0.85 (1/3) r1. Solved: r1 = 0.3744, r2 = 0.3658, r3 = 0.2597.
    let r1: f64 = 0.374430718882723;
    let r2: f64 = 0.3658290197957532;
    let k = (0.15 + 0.85 * r2) / 3.0;
    assert!((r2 - (k + 0.85 * (2.0 / 3.0) * r1)).abs() < 1e-5);
    assert!((r1 - (k + 0.85 * (k + 0.85 * r1 / 3.0))).abs() < 1e-5);
}

#[test]
fn parallel_edges_add_and_an_impl_weighs_point_seven() {
    // Two calls of 0.5 make 1.0; the impl's own confidence is ignored.
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 1)
        .node(3, 1)
        .call(1, 2, 0.5)
        .call(1, 2, 0.5)
        .implements(1, 3, 0.95);
    assert_scores(
        &ranked(&graph).symbols,
        &[
            (1, 0.25974019022001016),
            (2, 0.38961044277293333),
            (3, 0.3506493670070564),
        ],
        1e-9,
    );
}

#[test]
fn a_test_edge_carries_a_fifth_of_its_mass() {
    // File 1 holds 1 and the test 2; 1 calls 3 in file 2 and 2 calls 4 in
    // file 3. The test's row is normalised first, so its edge carries 0.2
    // of its damped score and the other 0.8 goes along the prior with the
    // dangling mass (PM ruling, SPEC-w7 G1b).
    let graph = Graph::default()
        .node(1, 1)
        .test_node(2, 1)
        .node(3, 2)
        .node(4, 3)
        .call(1, 3, 1.0)
        .call(2, 4, 1.0);
    let ranks = ranked(&graph);
    let k = 0.19920317219260925;
    assert_scores(
        &ranks.symbols,
        &[
            (1, k),
            (2, k),
            (3, 0.3685259315505785),
            (4, 0.2330677240642031),
        ],
        1e-9,
    );
    // With k = (0.15 + 0.85 (r3 + r4 + 0.8 r2)) / 4: r1 = r2 = k,
    // r3 = 1.85k, r4 = 1.17k, and the four sum to 5.02k = 1.
    let k = 1.0 / 5.02;
    assert_scores(
        &ranks.symbols,
        &[(1, k), (2, k), (3, 1.85 * k), (4, 1.17 * k)],
        1e-5,
    );
    // In the file graph the test's weight is 0.2 and file 1's row is
    // normalised after the sum: 1.0 to file 2 and 0.2 to file 3.
    assert_scores(
        &ranks.files,
        &[
            (1, 0.25974019022001016),
            (2, 0.44372304800331797),
            (3, 0.2965367617766717),
        ],
        1e-9,
    );
    // The same files with a non-test caller split evenly.
    let even = Graph::default()
        .node(1, 1)
        .node(2, 1)
        .node(3, 2)
        .node(4, 3)
        .call(1, 3, 1.0)
        .call(2, 4, 1.0);
    let ranks = ranked(&even);
    assert_eq!(ranks.files[1].1, ranks.files[2].1);
    assert_eq!(ranks.symbols[2].1, ranks.symbols[3].1);
}

#[test]
fn an_edge_inside_one_file_adds_no_file_edge() {
    // 1 and 2 share file 1, and 1 also calls 3 in file 2. File 1's row is
    // only its edge to file 2.
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 1)
        .node(3, 2)
        .call(1, 2, 1.0)
        .call(1, 3, 1.0);
    assert_scores(
        &ranked(&graph).files,
        &[(1, 0.3508771211772586), (2, 0.6491228788227412)],
        1e-9,
    );
}

#[test]
fn a_boosted_node_starts_with_twice_the_prior() {
    // 1 is boosted, so the prior is 1/2, 1/4, 1/4; both call 3.
    let graph = Graph::default()
        .boosted_node(1, 1)
        .node(2, 1)
        .node(3, 1)
        .call(1, 3, 1.0)
        .call(2, 3, 1.0);
    let ranks = ranked(&graph);
    assert_scores(
        &ranks.symbols,
        &[
            (1, 0.3053436192543968),
            (2, 0.1526718096271984),
            (3, 0.5419845711184049),
        ],
        1e-9,
    );
    // With k = 0.15 + 0.85 r3: r1 = k/2, r2 = k/4, r3 = 0.8875k, sum 1.6375k.
    let k = 1.0 / 1.6375;
    assert_scores(
        &ranks.symbols,
        &[(1, k / 2.0), (2, k / 4.0), (3, 0.8875 * k)],
        1e-5,
    );
    // With no edges at all, the scores are the prior.
    let alone = Graph::default().boosted_node(1, 1).node(2, 2);
    assert_scores(
        &ranked(&alone).symbols,
        &[(1, 2.0 / 3.0), (2, 1.0 / 3.0)],
        1e-15,
    );
}

#[test]
fn an_import_adds_point_three_to_the_file_edge() {
    // File 1 calls into file 2 at 1.0 and imports from file 3.
    let graph = Graph::default()
        .node(1, 1)
        .node(2, 2)
        .node(3, 3)
        .call(1, 2, 1.0)
        .import(1, 3);
    assert_scores(
        &ranked(&graph).files,
        &[
            (1, 0.2597401902200101),
            (2, 0.429570520481525),
            (3, 0.31068928929846457),
        ],
        1e-9,
    );
    // A file that only imports still has a file score.
    let ranks = ranked(&Graph::default().node(1, 2).import(9, 2));
    assert_eq!(
        ranks.files.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [2, 9]
    );
}

#[test]
fn the_iteration_cap_holds_on_a_slow_graph() {
    // A two-cycle with an uneven prior oscillates, and its L1 change
    // shrinks by exactly d per iteration. At d = 0.85 every graph's change
    // shrinks at least that fast, so the SPEC stop rule ends this run at
    // 83; at d = 0.99 it would need far more than 100.
    let edges = [(0, 1, 1.0), (1, 0, 1.0)];
    let spec = pagerank(2, &edges, &[2.0, 1.0], &[1.0; 2], &Iteration::SPEC);
    assert_eq!(spec.iterations, 83);
    assert!((spec.scores[0] - 0.5135133012395265).abs() < 1e-9);
    let slow = Iteration {
        damping: 0.99,
        ..Iteration::SPEC
    };
    let capped = pagerank(2, &edges, &[2.0, 1.0], &[1.0; 2], &slow);
    assert_eq!(capped.iterations, 100);
    assert!((capped.scores.iter().sum::<f64>() - 1.0).abs() < 1e-9);
}

#[test]
fn personalised_rank_puts_the_prior_on_the_focus() {
    let global = ranked(&chain());
    let focused = personalised(&chain(), &[1]).expect("personalised");
    assert_sums_to_one(&focused);
    // All prior mass on 10, the chain's head.
    let iterated = [
        (10, 0.3887271909446798),
        (20, 0.3304177346244974),
        (30, 0.2808550744308228),
    ];
    assert_scores(&focused.symbols, &iterated, 1e-9);
    // r10 = 0.15 + 0.85 r30, r20 = 0.85 r10, r30 = 0.85 r20.
    assert!((iterated[0].1 - 0.15 / (1.0 - 0.85_f64.powi(3))).abs() < 1e-5);
    assert_scores(
        &focused.files,
        &[(1, iterated[0].1), (2, iterated[1].1), (3, iterated[2].1)],
        1e-9,
    );
    assert!(focused.symbols[0].1 > global.symbols[0].1);
    assert!(focused.files[0].1 > global.files[0].1);
    // A focus with no symbol and no file in the graph keeps the global prior.
    assert_eq!(personalised(&chain(), &[99]).expect("personalised"), global);
}

#[test]
fn the_same_graph_in_any_order_gives_the_same_bits() {
    let graph = Graph::default()
        .boosted_node(5, 2)
        .node(3, 1)
        .test_node(8, 3)
        .node(1, 1)
        .call(3, 5, 0.95)
        .call(8, 3, 1.0)
        .call(1, 5, 0.9)
        .implements(5, 1, 1.0)
        .call(8, 5, 0.5)
        .import(1, 2)
        .import(3, 2);
    let first = ranked(&graph);
    assert_eq!(ranked(&graph), first);
    let mut reversed = graph.clone();
    reversed.nodes.reverse();
    reversed.edges.reverse();
    reversed.imports.reverse();
    assert_eq!(ranked(&reversed), first);
    let ids: Vec<i64> = first.symbols.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, [1, 3, 5, 8]);
}

#[test]
fn an_empty_graph_has_no_scores() {
    let ranks = rank(&Graph::default()).expect("rank");
    assert_eq!(ranks.symbols, []);
    assert_eq!(ranks.files, []);
    // An edge to a node the graph lacks is dropped.
    let ranks = ranked(&Graph::default().node(1, 1).call(1, 2, 1.0));
    assert_scores(&ranks.symbols, &[(1, 1.0)], 1e-12);
}
