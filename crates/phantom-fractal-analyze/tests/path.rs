//! `path` (SPEC-w9 G3a, D9) on hand-built graphs, against a brute-force
//! search on seeded random ones, and on an indexed fixture.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use phantom_fractal_analyze::{
    PATH_DEFAULT_DEPTH, PATH_MAX_DEPTH, PathEdge, PathEdgeKind, PathNode, PathSource, SymbolPath,
    path,
};
use phantom_fractal_store::{Store, StoreError};

/// A graph of named nodes, each keyed `k:<name>`.
#[derive(Default)]
struct Graph {
    names: BTreeMap<i64, String>,
    edges: Vec<PathEdge>,
}

impl Graph {
    fn node(&mut self, id: i64, name: &str) -> &mut Self {
        self.names.insert(id, name.to_owned());
        self
    }

    fn call(&mut self, from: i64, to: i64, confidence: f64) -> &mut Self {
        self.edge(from, to, PathEdgeKind::Calls, confidence)
    }

    fn member(&mut self, from: i64, to: i64) -> &mut Self {
        self.edge(from, to, PathEdgeKind::HasMember, 1.0)
    }

    fn edge(&mut self, from: i64, to: i64, kind: PathEdgeKind, confidence: f64) -> &mut Self {
        self.edges.push(PathEdge {
            from,
            to,
            kind,
            confidence,
        });
        self
    }

    fn path(&self, from: i64, to: i64, depth: u8) -> SymbolPath {
        path(self, from, to, depth).expect("path")
    }
}

impl PathSource for Graph {
    fn path_edges(&self) -> Result<Vec<PathEdge>, StoreError> {
        Ok(self.edges.clone())
    }

    fn path_node(&self, id: i64) -> Result<Option<PathNode>, StoreError> {
        Ok(self.names.get(&id).map(|name| PathNode {
            id,
            key: format!("k:{name}"),
            name: name.clone(),
            path: "src/lib.rs".to_owned(),
            line: Some(id),
        }))
    }
}

/// The names along a found path, `start` first.
fn names(found: &SymbolPath) -> Vec<String> {
    let mut out: Vec<String> = found.start.iter().map(|node| node.name.clone()).collect();
    out.extend(found.hops.iter().map(|hop| hop.name.clone()));
    out
}

/// A graph of nodes 1..=n named `n<id>`.
fn numbered(n: i64) -> Graph {
    let mut g = Graph::default();
    for id in 1..=n {
        g.node(id, &format!("n{id}"));
    }
    g
}

#[test]
fn the_shortest_path_wins_over_a_longer_surer_one() {
    let mut g = numbered(5);
    g.call(1, 2, 0.5).call(2, 5, 0.5); // 2 edges, 0.25
    g.call(1, 3, 1.0).call(3, 4, 1.0).call(4, 5, 1.0); // 3 edges, 1.0
    let found = g.path(1, 5, 10);
    assert!(found.found);
    assert_eq!(names(&found), ["n1", "n2", "n5"]);
    assert_eq!(found.confidence, Some(0.25));
    assert_eq!(found.farthest, None);
}

#[test]
fn among_the_shortest_the_highest_product_wins() {
    let mut g = numbered(4);
    g.call(1, 2, 0.9).call(2, 4, 0.5); // 0.45
    g.call(1, 3, 0.6).call(3, 4, 0.95); // 0.57
    let found = g.path(1, 4, 10);
    assert_eq!(names(&found), ["n1", "n3", "n4"]);
    assert_eq!(found.confidence, Some(0.6 * 0.95));
    let confidences: Vec<f64> = found.hops.iter().map(|hop| hop.confidence).collect();
    assert_eq!(confidences, [0.6, 0.95]);
}

#[test]
fn an_equal_product_is_broken_by_the_smaller_key_sequence() {
    let mut g = Graph::default();
    g.node(1, "a")
        .node(2, "zeta")
        .node(3, "beta")
        .node(4, "end");
    g.call(1, 2, 0.5).call(2, 4, 1.0);
    g.call(1, 3, 1.0).call(3, 4, 0.5);
    // Equal products; `k:beta` sorts before `k:zeta`, whatever the ids.
    assert_eq!(names(&g.path(1, 4, 10)), ["a", "beta", "end"]);
}

#[test]
fn a_cycle_is_walked_once() {
    let mut g = numbered(4);
    g.call(1, 2, 1.0)
        .call(2, 3, 1.0)
        .call(3, 1, 1.0)
        .call(3, 4, 1.0);
    assert_eq!(names(&g.path(3, 2, 10)), ["n3", "n1", "n2"]);
    assert_eq!(names(&g.path(1, 4, 10)), ["n1", "n2", "n3", "n4"]);
    // n4 reaches nothing, so a path back is none, and n4 is its own
    // farthest node.
    let back = g.path(4, 1, 10);
    assert!(!back.found);
    assert_eq!(back.farthest.map(|node| node.name), Some("n4".to_owned()));
}

#[test]
fn no_path_names_the_farthest_node_with_ties_by_key() {
    let mut g = Graph::default();
    g.node(1, "start")
        .node(2, "mid")
        .node(3, "deep-b")
        .node(4, "deep-a")
        .node(5, "away");
    g.call(1, 2, 1.0).call(2, 3, 1.0).call(2, 4, 1.0);
    let found = g.path(1, 5, 10);
    assert!(!found.found);
    assert!(found.hops.is_empty());
    assert_eq!(found.confidence, None);
    assert_eq!(found.start.map(|node| node.name), Some("start".to_owned()));
    assert_eq!(
        found.farthest.map(|node| node.name),
        Some("deep-a".to_owned())
    );
}

#[test]
fn from_equal_to_is_zero_hops() {
    let mut g = numbered(2);
    g.call(1, 2, 0.5).call(2, 1, 0.5);
    let found = g.path(1, 1, 1);
    assert!(found.found);
    assert!(found.hops.is_empty());
    assert_eq!(found.confidence, Some(1.0));
    assert_eq!(names(&found), ["n1"]);
}

#[test]
fn the_depth_cut_is_inclusive() {
    let mut g = numbered(4);
    g.call(1, 2, 1.0).call(2, 3, 1.0).call(3, 4, 1.0);
    assert_eq!(names(&g.path(1, 4, 3)), ["n1", "n2", "n3", "n4"]);
    let cut = g.path(1, 4, 2);
    assert!(!cut.found);
    assert_eq!(cut.farthest.map(|node| node.name), Some("n3".to_owned()));
    let one = g.path(1, 4, 1);
    assert_eq!(one.farthest.map(|node| node.name), Some("n2".to_owned()));
}

#[test]
fn has_member_edges_go_from_the_type_to_its_methods_only() {
    let mut g = Graph::default();
    g.node(1, "caller")
        .node(2, "Type")
        .node(3, "Type::m")
        .node(4, "other");
    g.call(1, 2, 0.9).member(2, 3).call(3, 4, 0.8);
    let found = g.path(1, 4, 10);
    let kinds: Vec<&str> = found
        .hops
        .iter()
        .map(|hop| hop.edge_kind.as_str())
        .collect();
    assert_eq!(kinds, ["calls", "has_member", "calls"]);
    assert_eq!(found.confidence, Some(0.9 * 1.0 * 0.8));
    // Never from a method back to its type.
    assert!(!g.path(3, 2, 10).found);
}

#[test]
fn parallel_edges_count_once_at_their_best() {
    let mut g = numbered(2);
    g.call(1, 2, 0.3)
        .call(1, 2, 0.9)
        .member(1, 2)
        .call(1, 1, 1.0);
    let found = g.path(1, 2, 10);
    assert_eq!(found.hops.len(), 1);
    assert_eq!(found.hops[0].edge_kind, PathEdgeKind::HasMember);
    assert_eq!(found.hops[0].confidence, 1.0);
    // At equal confidence, `calls` is kept over `has_member`, in either
    // order of the edges.
    for flip in [false, true] {
        let mut g = numbered(2);
        if flip {
            g.member(1, 2).call(1, 2, 1.0);
        } else {
            g.call(1, 2, 1.0).member(1, 2);
        }
        assert_eq!(
            g.path(1, 2, 1).hops[0].edge_kind,
            PathEdgeKind::Calls,
            "{flip}"
        );
    }
}

#[test]
fn a_symbol_without_a_row_finds_nothing() {
    let mut g = numbered(2);
    g.call(1, 2, 1.0).call(1, 99, 1.0);
    let found = g.path(1, 99, 10);
    assert!(!found.found);
    let none = g.path(99, 1, 10);
    assert_eq!((none.found, none.start, none.farthest), (false, None, None));
}

/// A fixed pseudo-random sequence.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

/// Every simple path from `from` to `to` of at most `depth` edges, best
/// edge per pair, by brute force: the shortest, then the highest product
/// (taken in path order), then the smallest key sequence.
fn brute(g: &Graph, from: i64, to: i64, depth: usize) -> Option<(Vec<i64>, f64)> {
    let mut best_edges: HashMap<(i64, i64), f64> = HashMap::new();
    for edge in &g.edges {
        if edge.from != edge.to {
            let slot = best_edges
                .entry((edge.from, edge.to))
                .or_insert(edge.confidence);
            *slot = slot.max(edge.confidence);
        }
    }
    let key = |id: i64| format!("k:{}", g.names[&id]);
    let mut found: Vec<(Vec<i64>, f64)> = Vec::new();
    let mut stack = vec![(vec![from], 1.0)];
    while let Some((walk, confidence)) = stack.pop() {
        let last = *walk.last().expect("non-empty");
        if last == to {
            found.push((walk, confidence));
            continue;
        }
        if walk.len() > depth {
            continue;
        }
        for (&(a, b), &c) in &best_edges {
            if a == last && !walk.contains(&b) {
                let mut next = walk.clone();
                next.push(b);
                stack.push((next, confidence * c));
            }
        }
    }
    found.into_iter().min_by(|(wa, ca), (wb, cb)| {
        wa.len()
            .cmp(&wb.len())
            .then(cb.total_cmp(ca))
            .then_with(|| {
                let ka: Vec<String> = wa.iter().map(|&id| key(id)).collect();
                let kb: Vec<String> = wb.iter().map(|&id| key(id)).collect();
                ka.cmp(&kb)
            })
    })
}

/// The node `from` reaches at the greatest edge distance within `depth`,
/// the smaller key on a tie, by relaxing distances until they settle.
fn farthest_brute(g: &Graph, from: i64, depth: usize) -> i64 {
    let mut distance: BTreeMap<i64, usize> = BTreeMap::from([(from, 0)]);
    loop {
        let mut changed = false;
        for edge in &g.edges {
            if let Some(&near) = distance.get(&edge.from)
                && near < depth
                && distance.get(&edge.to).is_none_or(|&far| far > near + 1)
            {
                distance.insert(edge.to, near + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let most = *distance.values().max().expect("from itself");
    *distance
        .iter()
        .filter(|(_, near)| **near == most)
        .map(|(id, _)| id)
        .min_by_key(|id| format!("k:{}", g.names[id]))
        .expect("one at most")
}

#[test]
fn random_graphs_agree_with_a_brute_force_search() {
    let confidences = [1.0, 0.95, 0.9, 0.5, 0.25];
    let mut lcg = Lcg(7);
    let mut compared = 0;
    for _ in 0..300 {
        let n = 3 + lcg.next(6) as i64;
        let mut g = Graph::default();
        for id in 1..=n {
            // Shuffled names, so key order is not id order.
            g.node(id, &format!("s{:02}-{id}", lcg.next(100)));
        }
        for _ in 0..lcg.next(3 * n as u64) + 1 {
            let (a, b) = (1 + lcg.next(n as u64) as i64, 1 + lcg.next(n as u64) as i64);
            let c = confidences[lcg.next(confidences.len() as u64) as usize];
            if lcg.next(5) == 0 {
                g.member(a, b);
            } else {
                g.call(a, b, c);
            }
        }
        let depth = 1 + lcg.next(5) as u8;
        for from in 1..=n {
            for to in 1..=n {
                let got = g.path(from, to, depth);
                match brute(&g, from, to, usize::from(depth)) {
                    Some((walk, confidence)) => {
                        assert!(got.found, "{from}->{to} depth {depth}");
                        let mut ids: Vec<i64> = vec![from];
                        ids.extend(got.hops.iter().map(|hop| hop.line.expect("line is the id")));
                        assert_eq!(ids, walk, "{from}->{to} depth {depth}");
                        assert_eq!(got.confidence, Some(confidence));
                    }
                    None => {
                        assert!(!got.found, "{from}->{to} depth {depth}");
                        let want = farthest_brute(&g, from, usize::from(depth));
                        assert_eq!(got.farthest.map(|node| node.id), Some(want), "{from}->{to}");
                    }
                }
                compared += 1;
            }
        }
    }
    assert!(compared > 5_000, "{compared}");
}

#[test]
fn the_depth_constants() {
    assert_eq!((PATH_DEFAULT_DEPTH, PATH_MAX_DEPTH), (10, 20));
}

#[test]
fn an_indexed_fixture_has_call_and_member_paths() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/resolve-self");
    let dir = std::env::temp_dir().join(format!("phantom-analyze-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut store = Store::open(&dir.join("graph.db")).expect("open store");
    let rust = phantom_fractal_lang_rust::RustLanguage::new().expect("query compiles");
    phantom_fractal_extract::index(&fixture, &mut store, &rust).expect("index");
    let id = |key: &str| {
        store
            .symbol_by_stable_key(key)
            .expect("lookup")
            .unwrap_or_else(|| panic!("no {key}"))
    };
    let bump = id("src/lib.rs#Counter::bump#method#0");
    let check = id("src/lib.rs#Counter::check#method#0");
    let counter = id("src/lib.rs#Counter#struct#0");
    let direct = path(&store, bump, check, PATH_DEFAULT_DEPTH).expect("path");
    assert!(direct.found);
    assert_eq!(direct.hops.len(), 1);
    assert_eq!(direct.hops[0].edge_kind, PathEdgeKind::Calls);
    assert_eq!(direct.hops[0].key, "src/lib.rs#Counter::check#method#0");
    let member = path(&store, counter, check, PATH_DEFAULT_DEPTH).expect("path");
    assert!(member.found);
    assert_eq!(member.hops[0].edge_kind, PathEdgeKind::HasMember);
    assert_eq!(member.hops.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
