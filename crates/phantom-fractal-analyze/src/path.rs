//! The call path between two symbols (SPEC-w9 G3a, D9).
//!
//! [`path`] searches the graph of outgoing `calls` edges and
//! `has_member` edges (a struct or enum to each method resolved to it),
//! breadth first from both ends: forward from `from` and backward from
//! `to`, each to `max_depth` edges. The shortest length is the least sum
//! of the two distances. Among the paths of that length it keeps the one
//! whose edge confidences have the highest product, and among those the
//! one whose sequence of stable keys is smallest. When no path is within
//! `max_depth`, it names the farthest node `from` reaches instead.
//!
//! It reads through [`PathSource`] and never writes.

use std::collections::{BTreeMap, HashMap, VecDeque};

use phantom_fractal_store::{Store, StoreError};

/// The fewest and most edges [`path`] may be asked to search.
pub const PATH_MIN_DEPTH: u8 = 1;
pub const PATH_MAX_DEPTH: u8 = 20;
/// The depth `path` searches when none is given.
pub const PATH_DEFAULT_DEPTH: u8 = 10;

/// The confidence of a `has_member` edge: the resolver placed it.
pub const HAS_MEMBER_CONFIDENCE: f64 = 1.0;

/// The kinds of edge a path follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathEdgeKind {
    /// A resolved call, from the caller to the callee.
    Calls,
    /// A struct or enum to one of its methods.
    HasMember,
}

impl PathEdgeKind {
    /// The stable name: `calls` or `has_member`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::HasMember => "has_member",
        }
    }
}

/// One edge of the path graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathEdge {
    pub from: i64,
    pub to: i64,
    pub kind: PathEdgeKind,
    pub confidence: f64,
}

/// A symbol as a path names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathNode {
    pub id: i64,
    /// The stable key, which breaks ties.
    pub key: String,
    pub name: String,
    /// The symbol's file, repo-relative.
    pub path: String,
    pub line: Option<i64>,
}

/// One step of a path: the edge taken, and the symbol it reaches.
#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub key: String,
    pub name: String,
    pub path: String,
    pub line: Option<i64>,
    pub edge_kind: PathEdgeKind,
    /// The edge's own confidence.
    pub confidence: f64,
}

/// What [`path`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolPath {
    /// A path of at most `max_depth` edges exists.
    pub found: bool,
    /// The symbol the path starts at, or none when `from` has no row.
    pub start: Option<PathNode>,
    /// The steps from `start` to `to`, in order: empty when `from` is
    /// `to`, and when nothing was found.
    pub hops: Vec<Hop>,
    /// The product of the hops' confidences, 1 for no hops; none when
    /// nothing was found.
    pub confidence: Option<f64>,
    /// When nothing was found: the node `from` reaches at the greatest
    /// depth within `max_depth`, ties broken by the smaller key; `from`
    /// itself when it reaches nothing.
    pub farthest: Option<PathNode>,
}

/// What [`path`] reads.
pub trait PathSource {
    /// Every `calls` and `has_member` edge.
    fn path_edges(&self) -> Result<Vec<PathEdge>, StoreError>;
    /// The symbol `id`, or none when it has no row.
    fn path_node(&self, id: i64) -> Result<Option<PathNode>, StoreError>;
}

impl PathSource for Store {
    fn path_edges(&self) -> Result<Vec<PathEdge>, StoreError> {
        let mut out: Vec<PathEdge> = self
            .rank_call_edges()?
            .into_iter()
            .filter(|edge| edge.kind == "call")
            .map(|edge| PathEdge {
                from: edge.src_id,
                to: edge.dst_id,
                kind: PathEdgeKind::Calls,
                confidence: edge.confidence,
            })
            .collect();
        for row in self.rank_input()? {
            if row.kind == "struct" || row.kind == "enum" {
                for member in self.members(row.id)? {
                    out.push(PathEdge {
                        from: row.id,
                        to: member.id,
                        kind: PathEdgeKind::HasMember,
                        confidence: HAS_MEMBER_CONFIDENCE,
                    });
                }
            }
        }
        Ok(out)
    }

    fn path_node(&self, id: i64) -> Result<Option<PathNode>, StoreError> {
        Ok(self.symbol_detail(id)?.map(|detail| PathNode {
            id: detail.id,
            key: detail.stable_key,
            name: detail.name,
            path: detail.path,
            line: detail.start_line,
        }))
    }
}

/// The best edge between two symbols: parallel edges collapse to the
/// highest confidence, then `calls` before `has_member`.
type Graph = BTreeMap<i64, BTreeMap<i64, (PathEdgeKind, f64)>>;

/// The path from `from` to `to` of at most `max_depth` edges, as the
/// module doc says. The caller keeps `max_depth` within
/// [`PATH_MIN_DEPTH`] to [`PATH_MAX_DEPTH`].
pub fn path(
    source: &impl PathSource,
    from: i64,
    to: i64,
    max_depth: u8,
) -> Result<SymbolPath, StoreError> {
    let mut nodes = Nodes::new(source);
    let Some(start) = nodes.get(from)? else {
        return Ok(not_found(None, None));
    };
    if from == to {
        return Ok(SymbolPath {
            found: true,
            start: Some(start),
            hops: Vec::new(),
            confidence: Some(1.0),
            farthest: None,
        });
    }
    if nodes.get(to)?.is_none() {
        return Ok(not_found(Some(start.clone()), Some(start)));
    }
    let (forward, backward) = graphs(source.path_edges()?);
    let depth = usize::from(max_depth);
    let ahead = distances(&forward, from, depth);
    let behind = distances(&backward, to, depth);
    let shortest = ahead
        .iter()
        .filter_map(|(id, near)| behind.get(id).map(|far| near + far))
        .filter(|&total| total <= depth)
        .min();
    let Some(length) = shortest else {
        let farthest = farthest(&ahead, &mut nodes)?;
        return Ok(not_found(Some(start), farthest));
    };

    // The layers of the shortest-path graph, best prefix first: each node
    // keeps the highest product, then the smaller key sequence.
    let mut best: HashMap<i64, Prefix> = HashMap::from([(
        from,
        Prefix {
            confidence: 1.0,
            keys: vec![start.key.clone()],
            back: None,
        },
    )]);
    let mut layer = vec![from];
    for step in 1..=length {
        let mut next: BTreeMap<i64, Prefix> = BTreeMap::new();
        for &node in &layer {
            let prefix = best[&node].clone();
            for (&reached, &(kind, confidence)) in forward.get(&node).into_iter().flatten() {
                if ahead.get(&reached) != Some(&step)
                    || behind.get(&reached) != Some(&(length - step))
                {
                    continue;
                }
                let Some(found) = nodes.get(reached)? else {
                    continue;
                };
                let mut keys = prefix.keys.clone();
                keys.push(found.key);
                let candidate = Prefix {
                    confidence: prefix.confidence * confidence,
                    keys,
                    back: Some((node, kind, confidence)),
                };
                match next.get(&reached) {
                    Some(known) if !candidate.beats(known) => {}
                    _ => {
                        next.insert(reached, candidate);
                    }
                }
            }
        }
        layer = next.keys().copied().collect();
        best.extend(next);
    }

    let Some(end) = best.get(&to).map(|end| end.confidence) else {
        // A node between the ends had no row, so no full path is known.
        let farthest = farthest(&ahead, &mut nodes)?;
        return Ok(not_found(Some(start), farthest));
    };
    let mut hops = Vec::new();
    let mut at = to;
    while let Some((back, kind, confidence)) = best[&at].back {
        let node = nodes.get(at)?.expect("a node on the path has a row");
        hops.push(Hop {
            key: node.key,
            name: node.name,
            path: node.path,
            line: node.line,
            edge_kind: kind,
            confidence,
        });
        at = back;
    }
    hops.reverse();
    Ok(SymbolPath {
        found: true,
        start: Some(start),
        hops,
        confidence: Some(end),
        farthest: None,
    })
}

/// The best way found to one node of a layer.
#[derive(Debug, Clone)]
struct Prefix {
    confidence: f64,
    /// The stable keys from `from` to this node.
    keys: Vec<String>,
    /// The node before, and the edge from it.
    back: Option<(i64, PathEdgeKind, f64)>,
}

impl Prefix {
    /// A higher product wins, then the smaller key sequence.
    fn beats(&self, other: &Self) -> bool {
        match self.confidence.total_cmp(&other.confidence) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => self.keys < other.keys,
        }
    }
}

fn not_found(start: Option<PathNode>, farthest: Option<PathNode>) -> SymbolPath {
    SymbolPath {
        found: false,
        start,
        hops: Vec::new(),
        confidence: None,
        farthest,
    }
}

/// The forward and backward graphs of `edges`, self-loops left out.
fn graphs(edges: Vec<PathEdge>) -> (Graph, Graph) {
    let mut forward = Graph::new();
    let mut backward = Graph::new();
    for edge in edges {
        if edge.from == edge.to {
            continue;
        }
        for (graph, a, b) in [
            (&mut forward, edge.from, edge.to),
            (&mut backward, edge.to, edge.from),
        ] {
            let slot = graph
                .entry(a)
                .or_default()
                .entry(b)
                .or_insert((edge.kind, edge.confidence));
            if edge.confidence > slot.1 || (edge.confidence == slot.1 && edge.kind < slot.0) {
                *slot = (edge.kind, edge.confidence);
            }
        }
    }
    (forward, backward)
}

/// Each node's edge distance from `origin` in `graph`, up to `depth`.
fn distances(graph: &Graph, origin: i64, depth: usize) -> HashMap<i64, usize> {
    let mut out = HashMap::from([(origin, 0)]);
    let mut queue = VecDeque::from([origin]);
    while let Some(node) = queue.pop_front() {
        let near = out[&node];
        if near == depth {
            continue;
        }
        for &next in graph.get(&node).into_iter().flat_map(BTreeMap::keys) {
            if let std::collections::hash_map::Entry::Vacant(slot) = out.entry(next) {
                slot.insert(near + 1);
                queue.push_back(next);
            }
        }
    }
    out
}

/// The node of `ahead` at the greatest distance, ties broken by the
/// smaller key (then id, should two keys be equal); nodes without a row
/// are passed over.
fn farthest(
    ahead: &HashMap<i64, usize>,
    nodes: &mut Nodes<'_, impl PathSource>,
) -> Result<Option<PathNode>, StoreError> {
    let Some(&most) = ahead.values().max() else {
        return Ok(None);
    };
    for depth in (0..=most).rev() {
        let mut at_depth: Vec<PathNode> = Vec::new();
        for (&id, &near) in ahead {
            if near == depth
                && let Some(node) = nodes.get(id)?
            {
                at_depth.push(node);
            }
        }
        if let Some(node) = at_depth
            .into_iter()
            .min_by(|a, b| a.key.cmp(&b.key).then(a.id.cmp(&b.id)))
        {
            return Ok(Some(node));
        }
    }
    Ok(None)
}

/// [`PathSource::path_node`], read once per id.
struct Nodes<'a, S> {
    source: &'a S,
    known: HashMap<i64, Option<PathNode>>,
}

impl<'a, S: PathSource> Nodes<'a, S> {
    fn new(source: &'a S) -> Self {
        Self {
            source,
            known: HashMap::new(),
        }
    }

    fn get(&mut self, id: i64) -> Result<Option<PathNode>, StoreError> {
        if let Some(node) = self.known.get(&id) {
            return Ok(node.clone());
        }
        let node = self.source.path_node(id)?;
        self.known.insert(id, node.clone());
        Ok(node)
    }
}
