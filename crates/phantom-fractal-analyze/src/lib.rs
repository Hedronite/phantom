//! Impact analysis over a stored call graph (SPEC-w5 G2): what breaks if
//! a symbol changes, the PageRank of every symbol and file
//! (SPEC-w7 G1b), the budgeted repo map built on it (SPEC-w7 G2), and
//! the token estimator the map and every budgeted reply share
//! (SPEC-w8 G0a).
//!
//! [`impact`] walks incoming `calls` edges breadth-first from a target,
//! groups what it reaches by depth with a path confidence, lists tests
//! apart, says why the answer may be incomplete, and gives a risk level.
//! It reads through [`ImpactSource`], so tests can run it on constructed
//! graphs, and it never writes. [`rank`] and [`personalised`] read
//! through [`RankSource`] the same way, and [`render`] through
//! [`MapSource`].
#![forbid(unsafe_code)]

mod estimate;
mod map;
mod path;
mod rank;
mod risk;
mod walk;

use phantom_fractal_store::{CallHit, Store, StoreError, SymbolFacts};

pub use estimate::estimate_tokens;
pub use map::{
    ALSO_CHANGES_MIN_LIFT, ALSO_CHANGES_PARTNERS, Detail, Map, MapError, MapRequest, MapSource,
    packing_budget, render,
};
pub use path::{
    HAS_MEMBER_CONFIDENCE, Hop, PATH_DEFAULT_DEPTH, PATH_MAX_DEPTH, PATH_MIN_DEPTH, PathEdge,
    PathEdgeKind, PathNode, PathSource, SymbolPath, path,
};
pub use rank::{
    Converged, EdgeKind, IMPL_WEIGHT, IMPORT_WEIGHT, Iteration, PRIOR_BOOST, RankEdge, RankNode,
    RankSource, Ranks, TEST_FACTOR, pagerank, personalised, rank,
};
pub use risk::{CompletenessReason, ReasonCount, Risk};

/// What [`impact`] reads.
pub trait ImpactSource {
    /// The edges into symbol `id`: each hit's `symbol` is a caller.
    fn callers(&self, id: i64) -> Result<Vec<CallHit>, StoreError>;
    fn facts(&self, id: i64) -> Result<Option<SymbolFacts>, StoreError>;
    /// `call` and `method_call` refs ending in `name` with no edge.
    fn unresolved_named(&self, name: &str) -> Result<i64, StoreError>;
    /// `macro_arg` refs ending in `name`.
    fn macro_args_named(&self, name: &str) -> Result<i64, StoreError>;
    /// The stored count of `.rs` files the index pass could not read.
    fn rs_skipped(&self) -> Result<Option<u64>, StoreError>;
}

impl ImpactSource for Store {
    fn callers(&self, id: i64) -> Result<Vec<CallHit>, StoreError> {
        Store::callers(self, id)
    }

    fn facts(&self, id: i64) -> Result<Option<SymbolFacts>, StoreError> {
        self.symbol_facts(id)
    }

    fn unresolved_named(&self, name: &str) -> Result<i64, StoreError> {
        Store::unresolved_named(self, name)
    }

    fn macro_args_named(&self, name: &str) -> Result<i64, StoreError> {
        Store::macro_args_named(self, name)
    }

    fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
        Store::rs_skipped(self)
    }
}

/// How far and how faintly [`impact`] walks. The caller refuses values
/// outside the ranges; [`ImpactOptions::in_range`] checks them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpactOptions {
    /// The deepest level reported, 1 to 5.
    pub max_depth: u8,
    /// The lowest path confidence reported, `0 < x <= 1`.
    pub min_confidence: f64,
    /// When false, tests are left out entirely. They are never expanded.
    pub include_tests: bool,
}

impl ImpactOptions {
    pub const MAX_DEPTH: u8 = 5;

    /// True when `max_depth` is 1 to 5 and `min_confidence` is in
    /// `(0, 1]`. A NaN confidence is out of range.
    pub fn in_range(&self) -> bool {
        (1..=Self::MAX_DEPTH).contains(&self.max_depth)
            && self.min_confidence > 0.0
            && self.min_confidence <= 1.0
    }
}

impl Default for ImpactOptions {
    /// Depth 3, confidence 0.2, tests included.
    fn default() -> Self {
        Self {
            max_depth: 3,
            min_confidence: 0.2,
            include_tests: true,
        }
    }
}

/// The edge a hit was first reached by, at its best confidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    /// The symbol one level nearer the target that this hit calls.
    pub callee: i64,
    /// The edge's reason text, such as `local` or `crate-path`.
    pub reason: String,
    /// The call site.
    pub site_file: String,
    pub site_line: Option<i64>,
}

/// One reached symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub symbol: SymbolFacts,
    /// The level the symbol was first reached at, from 1.
    pub depth: u8,
    /// The best product of edge confidences on a path to the target.
    pub confidence: f64,
    pub via: Via,
}

/// The non-test hits of one depth, sorted by file, then line, then name.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub depth: u8,
    /// Every hit at this depth, kept or not.
    pub total: usize,
    /// True when `hits` holds the first [`Group::MAX_HITS`] of more.
    pub truncated: bool,
    pub hits: Vec<Hit>,
}

impl Group {
    pub const MAX_HITS: usize = 100;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Non-test symbols at depth 1.
    pub direct: u64,
    /// Every non-test symbol reported.
    pub total: u64,
    /// Distinct packages among the non-test symbols reported.
    pub packages: u64,
    /// Test symbols reported.
    pub tests: u64,
    /// Symbols reached below `min_confidence`: not reported, not expanded.
    pub below_floor: u64,
}

/// What [`impact`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    /// The target's facts, or none when it has no row.
    pub target: Option<SymbolFacts>,
    /// One group per depth that reported a hit, nearest first.
    pub groups: Vec<Group>,
    /// Reached tests, sorted by depth, then file, line, and name.
    pub tests: Vec<Hit>,
    pub counts: Counts,
    /// Why the answer may be incomplete, in the fixed order.
    pub reasons: Vec<ReasonCount>,
    /// True only when `reasons` is empty.
    pub exact: bool,
    pub risk: Risk,
}

/// What breaks if `target` changes, at `options`.
///
/// A target with no row (the index changed after the caller found it)
/// gives no hits, no reasons, `exact` false, and risk `unknown`.
pub fn impact(
    source: &impl ImpactSource,
    target: i64,
    options: &ImpactOptions,
) -> Result<Impact, StoreError> {
    let Some(facts) = source.facts(target)? else {
        return Ok(Impact {
            target: None,
            groups: Vec::new(),
            tests: Vec::new(),
            counts: Counts::default(),
            reasons: Vec::new(),
            exact: false,
            risk: Risk::Unknown,
        });
    };
    let walked = walk::walk(source, std::slice::from_ref(&facts), options)?;
    let reasons = risk::reasons(source, &facts)?;
    let risk = risk::risk(&walked.counts, &reasons);
    Ok(Impact {
        target: Some(facts),
        groups: walked.groups,
        tests: walked.tests,
        counts: walked.counts,
        exact: reasons.is_empty(),
        reasons,
        risk,
    })
}

/// What a diff tells [`impact_many`] beyond the symbols it walks from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffState {
    /// The freshness probe says the index is stale: the reason
    /// `stale_index`.
    pub stale_index: bool,
    /// Changed symbols the index does not hold: the reason
    /// `unindexed_changes`, counted.
    pub unindexed_changes: u64,
}

/// What [`impact_many`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct ManyImpact {
    /// The sources the walk started from, in hit order. A source with
    /// no row is left out.
    pub sources: Vec<SymbolFacts>,
    /// One group per depth that reported a hit, nearest first.
    pub groups: Vec<Group>,
    /// Reached tests, sorted by depth, then file, line, and name.
    pub tests: Vec<Hit>,
    pub counts: Counts,
    /// Why the answer may be incomplete, in the fixed order.
    pub reasons: Vec<ReasonCount>,
    /// True only when `reasons` is empty.
    pub exact: bool,
    pub risk: Risk,
}

/// What breaks if every symbol in `sources` changes, walked from all of
/// them at once (SPEC-w9 G2a): each reached symbol is placed once, at its
/// nearest depth from any source, by its best path. Groups, tests, counts
/// and risk are those of [`impact`]; the reasons are [`impact`]'s summed
/// over the sources, plus `stale_index` and `unindexed_changes` from
/// `diff`. A source is never a hit.
pub fn impact_many(
    source: &impl ImpactSource,
    sources: &[i64],
    options: &ImpactOptions,
    diff: &DiffState,
) -> Result<ManyImpact, StoreError> {
    let mut targets = Vec::new();
    for &id in sources {
        if let Some(facts) = source.facts(id)?
            && !targets.iter().any(|known: &SymbolFacts| known.id == id)
        {
            targets.push(facts);
        }
    }
    targets.sort_by(walk::order);
    let walked = walk::walk(source, &targets, options)?;
    let reasons = risk::reasons_of(source, &targets, diff)?;
    let risk = risk::risk(&walked.counts, &reasons);
    Ok(ManyImpact {
        sources: targets,
        groups: walked.groups,
        tests: walked.tests,
        counts: walked.counts,
        exact: reasons.is_empty(),
        reasons,
        risk,
    })
}
