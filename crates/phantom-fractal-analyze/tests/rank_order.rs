//! The rank does not depend on the row ids a build gave the graph
//! (SPEC-w9 G1): `fract watch` re-indexes files into new ids, and its
//! database must equal a fresh index's, rank scores to the last bit.

use std::collections::BTreeMap;

use phantom_fractal_analyze::{EdgeKind, RankEdge, RankNode, RankSource, rank};
use phantom_fractal_store::StoreError;

/// Files in walk order: `a/b.rs` comes before `a.rs`.
const FILES: [&str; 6] = [
    "Cargo.rs",
    "a/b.rs",
    "a.rs",
    "src/lib.rs",
    "src/m/x.rs",
    "z.rs",
];
const PER_FILE: usize = 5;

/// A symbol as `(file, position in the file)`.
type Key = (usize, usize);
/// The edges by `(from, to, is impl, confidence)`, and the imports.
type Shape = (Vec<(Key, Key, bool, f64)>, Vec<(usize, usize)>);
/// A build, and its symbol and file ids back to natural keys.
type Build = (Built, BTreeMap<i64, Key>, BTreeMap<i64, usize>);
/// Every symbol's and file's score as raw bits, by natural key.
type Scores = (Vec<(Key, u64)>, Vec<(usize, u64)>);

/// A graph whose ids are assigned per build.
struct Built {
    paths: Vec<(i64, String)>,
    nodes: Vec<RankNode>,
    edges: Vec<RankEdge>,
    imports: Vec<(i64, i64)>,
    with_paths: bool,
}

impl RankSource for Built {
    fn graph_epoch(&self) -> Result<u64, StoreError> {
        Ok(1)
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
    fn file_paths(&self) -> Result<Vec<(i64, String)>, StoreError> {
        Ok(if self.with_paths {
            self.paths.clone()
        } else {
            Vec::new()
        })
    }
}

/// A fixed pseudo-random sequence, so every run sees the same graph.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % below
    }
}

/// The graph's edges by `(file, position)` ends, and its imports by file.
fn shape(seed: u64) -> Shape {
    let mut lcg = Lcg(seed);
    let confidences = [1.0, 0.95, 0.9, 0.5, 0.7];
    let edges = (0..60)
        .map(|_| {
            let from = (lcg.next(FILES.len()), lcg.next(PER_FILE));
            let to = (lcg.next(FILES.len()), lcg.next(PER_FILE));
            (
                from,
                to,
                lcg.next(4) == 0,
                confidences[lcg.next(confidences.len())],
            )
        })
        .collect();
    let imports = (0..8)
        .map(|_| (lcg.next(FILES.len()), lcg.next(FILES.len())))
        .collect();
    (edges, imports)
}

/// The graph built with each file's ids in `order`: the files are stored
/// in that order, and each file's symbols get consecutive ids.
fn build(seed: u64, order: &[usize], with_paths: bool) -> Build {
    let (edges, imports) = shape(seed);
    let mut file_ids = BTreeMap::new();
    let mut symbol_ids = BTreeMap::new();
    let mut next = 100;
    for &file in order {
        file_ids.insert(file, 10 + next);
        for position in 0..PER_FILE {
            symbol_ids.insert((file, position), next);
            next += 1;
        }
    }
    let nodes = order
        .iter()
        .flat_map(|&file| (0..PER_FILE).map(move |position| (file, position)))
        .map(|(file, position)| RankNode {
            id: symbol_ids[&(file, position)],
            file_id: file_ids[&file],
            is_test: position == 4,
            boosted: position == 0,
        })
        .collect();
    let built = Built {
        paths: order
            .iter()
            .map(|&file| (file_ids[&file], FILES[file].to_owned()))
            .collect(),
        nodes,
        edges: edges
            .iter()
            .map(|&(from, to, implemented, confidence)| RankEdge {
                kind: if implemented {
                    EdgeKind::Impl
                } else {
                    EdgeKind::Call
                },
                source: symbol_ids[&from],
                target: symbol_ids[&to],
                confidence,
            })
            .collect(),
        imports: imports
            .iter()
            .map(|&(from, to)| (file_ids[&from], file_ids[&to]))
            .collect(),
        with_paths,
    };
    let by_symbol = symbol_ids.iter().map(|(&key, &id)| (id, key)).collect();
    let by_file = file_ids.iter().map(|(&file, &id)| (id, file)).collect();
    (built, by_symbol, by_file)
}

/// Every score by natural key, as raw bits.
fn scores(seed: u64, order: &[usize], with_paths: bool) -> Scores {
    let (built, by_symbol, by_file) = build(seed, order, with_paths);
    let ranks = rank(&built).expect("rank");
    let mut symbols: Vec<_> = ranks
        .symbols
        .iter()
        .map(|&(id, score)| (by_symbol[&id], score.to_bits()))
        .collect();
    symbols.sort();
    let mut files: Vec<_> = ranks
        .files
        .iter()
        .map(|&(id, score)| (by_file[&id], score.to_bits()))
        .collect();
    files.sort();
    (symbols, files)
}

#[test]
fn re_indexed_files_with_new_ids_rank_to_the_same_bits() {
    let walk: Vec<usize> = (0..FILES.len()).collect();
    for seed in 0..40 {
        let fresh = scores(seed, &walk, true);
        // Files 1 and 3 written again: their rows now come last.
        let reindexed = scores(seed, &[0, 2, 4, 5, 1, 3], true);
        // Every file written again, in reverse.
        let reversed = scores(seed, &[5, 4, 3, 2, 1, 0], true);
        assert_eq!(reindexed, fresh, "seed {seed}");
        assert_eq!(reversed, fresh, "seed {seed}");
    }
}

#[test]
fn a_fresh_build_ranks_as_plain_id_order_did() {
    // A fresh index stores files in walk order, so its id order is walk
    // order and the scores are the ones id order gave.
    let walk: Vec<usize> = (0..FILES.len()).collect();
    for seed in 0..40 {
        assert_eq!(
            scores(seed, &walk, true),
            scores(seed, &walk, false),
            "seed {seed}"
        );
    }
}

#[test]
fn without_paths_a_re_indexed_build_may_differ_in_the_last_bit() {
    // What W9 fixed: id order makes the sums depend on the build.
    let walk: Vec<usize> = (0..FILES.len()).collect();
    let differs =
        (0..40).any(|seed| scores(seed, &walk, false) != scores(seed, &[5, 4, 3, 2, 1, 0], false));
    assert!(differs);
}
