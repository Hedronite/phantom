//! `path`: a call path between two symbols (SPEC-w9 G3b, D9), from
//! `phantom_fractal_analyze::path` over the stored `calls` and
//! `has_member` edges. Nothing is written.

use std::path::PathBuf;

use phantom_fractal_analyze::{Hop, PATH_MAX_DEPTH, PATH_MIN_DEPTH, PathNode, path};
use phantom_fractal_proto::ErrorKind;
use phantom_fractal_store::Store;
use serde::Serialize;

use super::budget::{self, Shed, Tokens};
use super::lookup::{Names, Query, Retry, current_index, lookup_keyed};
use super::{Failure, Reply, answer};

/// What `path` is asked for.
pub(crate) struct PathRequest {
    /// Where the path starts: a stable key, or a name as `show` takes it.
    pub(crate) from: String,
    /// Where it ends, the same way.
    pub(crate) to: String,
    /// The most edges walked, [`PATH_MIN_DEPTH`] to [`PATH_MAX_DEPTH`].
    pub(crate) max_depth: u8,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The most estimated tokens the reply may cost. No hop is dropped.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of `path`, with its keys in this order.
#[derive(Clone, Serialize)]
pub(crate) struct PathResult {
    found: bool,
    /// The symbol the path starts at.
    from: NodeOut,
    /// The steps after `from`, each the symbol reached with the kind and
    /// confidence of the edge into it. None for `from == to`.
    hops: Vec<HopOut>,
    /// The product of the hops' confidences, 1 for no hops, null when
    /// nothing was found.
    confidence: Option<f64>,
    /// When nothing was found: the symbol `from` reaches at the greatest
    /// depth within `max_depth`, ties by key, or `from` itself.
    farthest: Option<NodeOut>,
    max_depth: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

/// A path is never cut: it fits whole or the reply is `budget_too_small`.
impl Shed for PathResult {
    fn sheddable(&self) -> usize {
        0
    }

    fn shed(&mut self, _count: usize) {}

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// One symbol on or off the path.
#[derive(Clone, Serialize)]
struct NodeOut {
    key: String,
    name: String,
    path: String,
    line: Option<i64>,
}

/// One step: the symbol reached and the edge into it.
#[derive(Clone, Serialize)]
struct HopOut {
    key: String,
    name: String,
    path: String,
    line: Option<i64>,
    edge_kind: &'static str,
    confidence: f64,
}

/// The best shortest path from `request.from` to `request.to` in the
/// index of `request.repo`, fitted to `request.max_tokens`.
pub(crate) fn symbol_path(request: &PathRequest) -> Result<Reply<PathResult>, Failure> {
    checked(request)?;
    let (store, names) = current_index(&request.repo)?;
    let (from, to) = ends(&store, &names, request)?;
    let found = path(&store, from, to, request.max_depth)?;
    let start = found
        .start
        .ok_or_else(|| Failure::new(ErrorKind::NotFound, "the symbol left the index"))?;
    let result = PathResult {
        found: found.found,
        from: node_out(start),
        hops: found.hops.into_iter().map(hop_out).collect(),
        confidence: found.confidence,
        farthest: found.farthest.map(node_out),
        max_depth: request.max_depth,
        tokens: None,
    };
    answer(result, &store, &request.repo, request.max_tokens)
}

/// `Usage` unless `max_depth` and `max_tokens` are in range.
fn checked(request: &PathRequest) -> Result<(), Failure> {
    if !(PATH_MIN_DEPTH..=PATH_MAX_DEPTH).contains(&request.max_depth) {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!("max_depth must be {PATH_MIN_DEPTH} to {PATH_MAX_DEPTH}"),
        ));
    }
    budget::check(request.max_tokens)
}

/// FROM and then TO, each looked up with the retry that keeps the other.
fn ends(store: &Store, names: &Names, request: &PathRequest) -> Result<(i64, i64), Failure> {
    let end = |text, retry| Query {
        repo: &request.repo,
        text,
        retry,
    };
    let from = end(&request.from, Retry::PathFrom(&request.to));
    let from = resolve(store, names, &from)?;
    let to = end(&request.to, Retry::PathTo(&request.from));
    Ok((from, resolve(store, names, &to)?))
}

/// The symbol `query` names: its exact stable key, or else a name as
/// `show` looks it up, `NotFound` when none and `Ambiguous`, with keyed
/// candidates, when more than one.
fn resolve(store: &Store, names: &Names, query: &Query) -> Result<i64, Failure> {
    if let Some(id) = store.symbol_by_stable_key(query.text)? {
        return Ok(id);
    }
    Ok(lookup_keyed(store, names, query)?.id)
}

fn node_out(node: PathNode) -> NodeOut {
    NodeOut {
        key: node.key,
        name: node.name,
        path: node.path,
        line: node.line,
    }
}

fn hop_out(hop: Hop) -> HopOut {
    HopOut {
        key: hop.key,
        name: hop.name,
        path: hop.path,
        line: hop.line,
        edge_kind: hop.edge_kind.as_str(),
        confidence: hop.confidence,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{PATH_MAX_DEPTH, PATH_MIN_DEPTH, PathRequest, checked};

    #[test]
    fn a_request_out_of_range_is_refused_before_any_read() {
        let request = |max_depth: u8, max_tokens: Option<u64>| PathRequest {
            from: "a".to_owned(),
            to: "b".to_owned(),
            max_depth,
            repo: PathBuf::from("."),
            max_tokens,
        };
        assert!(checked(&request(PATH_MIN_DEPTH, None)).is_ok());
        assert!(checked(&request(PATH_MAX_DEPTH, Some(100_000))).is_ok());
        for bad in [
            request(PATH_MIN_DEPTH - 1, None),
            request(PATH_MAX_DEPTH + 1, None),
            request(PATH_MIN_DEPTH, Some(0)),
        ] {
            assert!(checked(&bad).is_err());
        }
    }
}
