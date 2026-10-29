//! PageRank over the stored graph (SPEC-w7 G1b).
//!
//! [`rank`] scores every non-module symbol and every file. The symbol
//! graph weighs a `call` edge by its confidence and an `impl` edge by
//! [`IMPL_WEIGHT`], and an edge out of a test symbol by [`TEST_FACTOR`]
//! more; parallel edges add up and each row is normalised. The prior is
//! uniform but [`PRIOR_BOOST`] times as heavy on public items of a lib
//! target and on a bin's `fn main`. The file graph sums the symbol edge
//! weights between two files and adds [`IMPORT_WEIGHT`] per `import` edge,
//! under a uniform prior. [`personalised`] moves all prior mass onto the
//! focus files. Both read through [`RankSource`], never write, and give
//! the same output for the same input: nodes and edges are taken in id
//! order.

use std::collections::{BTreeMap, BTreeSet};

use phantom_fractal_store::{Store, StoreError};

/// What [`rank`] reads.
pub trait RankSource {
    /// The stored `graph_epoch`, which [`Ranks::epoch`] carries.
    fn graph_epoch(&self) -> Result<u64, StoreError>;
    /// Every non-module symbol.
    fn nodes(&self) -> Result<Vec<RankNode>, StoreError>;
    /// Every `call` and `impl` edge between two symbols.
    fn edges(&self) -> Result<Vec<RankEdge>, StoreError>;
    /// Every `import` edge as `(importing file, imported symbol's file)`.
    fn imports(&self) -> Result<Vec<(i64, i64)>, StoreError>;
}

impl RankSource for Store {
    fn graph_epoch(&self) -> Result<u64, StoreError> {
        Store::graph_epoch(self)
    }

    fn nodes(&self) -> Result<Vec<RankNode>, StoreError> {
        Ok(self
            .rank_input()?
            .into_iter()
            .map(|row| RankNode {
                id: row.id,
                file_id: row.file_id,
                is_test: row.is_test,
                boosted: row.is_main
                    || (row.visibility == "public" && row.target_kind.as_deref() == Some("lib")),
            })
            .collect())
    }

    fn edges(&self) -> Result<Vec<RankEdge>, StoreError> {
        Ok(self
            .rank_call_edges()?
            .into_iter()
            .filter_map(|edge| {
                let kind = match edge.kind.as_str() {
                    "call" => EdgeKind::Call,
                    "impl" => EdgeKind::Impl,
                    _ => return None,
                };
                Some(RankEdge {
                    kind,
                    source: edge.src_id,
                    target: edge.dst_id,
                    confidence: edge.confidence,
                })
            })
            .collect())
    }

    fn imports(&self) -> Result<Vec<(i64, i64)>, StoreError> {
        Ok(self
            .rank_import_edges()?
            .into_iter()
            .map(|edge| (edge.src_file_id, edge.dst_file_id))
            .collect())
    }
}

/// One symbol of the rank graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankNode {
    pub id: i64,
    pub file_id: i64,
    /// Edges out of a test symbol weigh [`TEST_FACTOR`] times as much.
    pub is_test: bool,
    /// A public item of a lib target, or a bin's `fn main`: its prior
    /// weighs [`PRIOR_BOOST`] times as much.
    pub boosted: bool,
}

/// The edge kinds the symbol graph counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Weighs its confidence.
    Call,
    /// Weighs [`IMPL_WEIGHT`], whatever its confidence.
    Impl,
}

/// One symbol-to-symbol edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankEdge {
    pub kind: EdgeKind,
    pub source: i64,
    pub target: i64,
    pub confidence: f64,
}

/// The weight of an `impl` edge.
pub const IMPL_WEIGHT: f64 = 0.7;
/// The factor on an edge out of a test symbol.
pub const TEST_FACTOR: f64 = 0.2;
/// The prior weight of a boosted node, against 1 for the rest.
pub const PRIOR_BOOST: f64 = 2.0;
/// The file-graph weight of one `import` edge.
pub const IMPORT_WEIGHT: f64 = 0.3;

/// The damping and the stop rule of one PageRank run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Iteration {
    pub damping: f64,
    /// The run stops once an iteration changes the scores by less than
    /// this, in L1.
    pub tolerance: f64,
    /// The run stops after this many iterations whatever the change.
    pub max_iterations: u32,
}

impl Iteration {
    /// SPEC-w7 G1b: d = 0.85, L1 below 1e-6, at most 100 iterations.
    pub const SPEC: Self = Self {
        damping: 0.85,
        tolerance: 1e-6,
        max_iterations: 100,
    };
}

/// The scores of one run, by node index, and how many iterations it took.
#[derive(Debug, Clone, PartialEq)]
pub struct Converged {
    pub scores: Vec<f64>,
    pub iterations: u32,
}

/// Every symbol's and every file's score, each list in id order and
/// summing to 1, and the graph epoch they were computed from.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranks {
    pub symbols: Vec<(i64, f64)>,
    pub files: Vec<(i64, f64)>,
    pub epoch: u64,
}

/// The global rank of `source`'s graph.
pub fn rank(source: &impl RankSource) -> Result<Ranks, StoreError> {
    compute(source, None)
}

/// The rank with all prior mass on `focus_files`: on their symbols in the
/// symbol graph, still boosted as in [`rank`], and spread evenly over the
/// files themselves in the file graph. A graph none of whose nodes is in
/// the focus keeps its global prior.
pub fn personalised(source: &impl RankSource, focus_files: &[i64]) -> Result<Ranks, StoreError> {
    compute(source, Some(&focus_files.iter().copied().collect()))
}

fn compute(source: &impl RankSource, focus: Option<&BTreeSet<i64>>) -> Result<Ranks, StoreError> {
    let mut nodes = source.nodes()?;
    nodes.sort_by_key(|node| node.id);
    let mut edges = source.edges()?;
    edges.sort_by(|a, b| {
        (a.source, a.target)
            .cmp(&(b.source, b.target))
            .then(a.confidence.total_cmp(&b.confidence))
    });
    let mut imports = source.imports()?;
    imports.sort_unstable();
    let index: BTreeMap<i64, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.id, position))
        .collect();

    // Symbol edges by node index, parallel edges summed; the file graph
    // gets the same weight between two different files.
    let mut symbol_weights: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut file_weights: BTreeMap<(i64, i64), f64> = BTreeMap::new();
    for edge in &edges {
        let (Some(&from), Some(&to)) = (index.get(&edge.source), index.get(&edge.target)) else {
            continue;
        };
        let weight = match edge.kind {
            EdgeKind::Call => edge.confidence,
            EdgeKind::Impl => IMPL_WEIGHT,
        };
        *symbol_weights.entry((from, to)).or_default() += weight;
        let files = (nodes[from].file_id, nodes[to].file_id);
        if files.0 != files.1 {
            *file_weights.entry(files).or_default() += weight * retain(&nodes[from]);
        }
    }
    for &(from, to) in &imports {
        if from != to {
            *file_weights.entry((from, to)).or_default() += IMPORT_WEIGHT;
        }
    }

    let in_focus = |file: i64| focus.is_none_or(|focus| focus.contains(&file));
    let symbol_prior: Vec<f64> = nodes
        .iter()
        .map(|node| match (in_focus(node.file_id), node.boosted) {
            (false, _) => 0.0,
            (true, true) => PRIOR_BOOST,
            (true, false) => 1.0,
        })
        .collect();
    let global: Vec<f64> = nodes
        .iter()
        .map(|node| if node.boosted { PRIOR_BOOST } else { 1.0 })
        .collect();
    let symbols = pagerank(
        nodes.len(),
        &edge_list(&symbol_weights),
        &prior_or(symbol_prior, global),
        &nodes.iter().map(retain).collect::<Vec<_>>(),
        &Iteration::SPEC,
    );

    let files: Vec<i64> = nodes
        .iter()
        .map(|node| node.file_id)
        .chain(imports.iter().flat_map(|&(from, to)| [from, to]))
        .collect::<BTreeSet<i64>>()
        .into_iter()
        .collect();
    let file_index: BTreeMap<i64, usize> = files
        .iter()
        .enumerate()
        .map(|(position, file)| (*file, position))
        .collect();
    let file_edges: Vec<(usize, usize, f64)> = file_weights
        .iter()
        .map(|(&(from, to), &weight)| (file_index[&from], file_index[&to], weight))
        .collect();
    let file_prior: Vec<f64> = files
        .iter()
        .map(|file| if in_focus(*file) { 1.0 } else { 0.0 })
        .collect();
    let file_scores = pagerank(
        files.len(),
        &file_edges,
        &prior_or(file_prior, vec![1.0; files.len()]),
        &vec![1.0; files.len()],
        &Iteration::SPEC,
    );

    Ok(Ranks {
        symbols: nodes
            .iter()
            .map(|node| node.id)
            .zip(symbols.scores)
            .collect(),
        files: files.into_iter().zip(file_scores.scores).collect(),
        epoch: source.graph_epoch()?,
    })
}

/// The share of a symbol's out-weight its edges keep: [`TEST_FACTOR`]
/// for a test symbol, else all of it.
fn retain(node: &RankNode) -> f64 {
    if node.is_test { TEST_FACTOR } else { 1.0 }
}

fn edge_list(weights: &BTreeMap<(usize, usize), f64>) -> Vec<(usize, usize, f64)> {
    weights
        .iter()
        .map(|(&(from, to), &weight)| (from, to, weight))
        .collect()
}

/// `prior`, or `fallback` when `prior` has no mass.
fn prior_or(prior: Vec<f64>, fallback: Vec<f64>) -> Vec<f64> {
    if prior.iter().sum::<f64>() > 0.0 {
        prior
    } else {
        fallback
    }
}

/// PageRank over `n` nodes: `r ← (1−d)·p + d·(Wᵀr + dangling·p)`.
///
/// `W` is `edges` with each row normalised to sum to `retain` of its node,
/// and `p` is `prior` normalised. `dangling` is the score of the nodes with
/// no out-weight, plus the `1 − retain` share of every other node's score:
/// mass no edge carries goes along the prior, so the scores sum to 1. The
/// run starts at `p` and stops at the first iteration whose L1 change is
/// below the tolerance, or at the iteration cap. Edges are `(from, to,
/// weight)` with a positive weight. No nodes, a slice of the wrong length,
/// or a prior with no mass give all-zero scores and no iterations.
pub fn pagerank(
    n: usize,
    edges: &[(usize, usize, f64)],
    prior: &[f64],
    retain: &[f64],
    iteration: &Iteration,
) -> Converged {
    let total: f64 = prior.iter().sum();
    if n == 0 || prior.len() != n || retain.len() != n || total <= 0.0 {
        return Converged {
            scores: vec![0.0; n],
            iterations: 0,
        };
    }
    let p: Vec<f64> = prior.iter().map(|weight| weight / total).collect();
    let mut out_weight = vec![0.0; n];
    for &(from, _, weight) in edges {
        out_weight[from] += weight;
    }
    let d = iteration.damping;
    let mut scores = p.clone();
    let mut iterations = 0;
    while iterations < iteration.max_iterations {
        iterations += 1;
        let dangling: f64 = (0..n)
            .map(|node| {
                if out_weight[node] <= 0.0 {
                    scores[node]
                } else {
                    scores[node] * (1.0 - retain[node])
                }
            })
            .sum();
        let mut next: Vec<f64> = p
            .iter()
            .map(|share| (1.0 - d) * share + d * dangling * share)
            .collect();
        for &(from, to, weight) in edges {
            next[to] += d * scores[from] * retain[from] * weight / out_weight[from];
        }
        let change: f64 = next.iter().zip(&scores).map(|(a, b)| (a - b).abs()).sum();
        scores = next;
        if change < iteration.tolerance {
            break;
        }
    }
    Converged { scores, iterations }
}
