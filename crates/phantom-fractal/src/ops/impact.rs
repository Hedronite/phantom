//! `impact`: what breaks if one symbol changes, from the stored call graph.
//!
//! The lookup and its errors are those of `callers`. The walk, the
//! completeness reasons, and the risk level come from
//! `phantom-fractal-analyze`. Every path is relative to the indexed tree.

use std::cmp::Reverse;
use std::path::PathBuf;

use phantom_fractal_analyze::{Counts, Group, Hit, ImpactOptions, ReasonCount};
use phantom_fractal_proto::ErrorKind;
use serde::Serialize;

use super::budget::{self, Shed, Tokens, drop_last};
use super::lookup::{Names, SymbolOut, current_index, lookup, symbol_out};
use super::{Failure, Reply};

/// What `impact` is asked for.
pub(crate) struct ImpactRequest {
    /// A bare, in-file, or module-qualified name.
    pub(crate) symbol: String,
    /// The indexed tree.
    pub(crate) repo: PathBuf,
    /// The walk's depth, confidence floor, and whether tests count. Out
    /// of range is `Usage`.
    pub(crate) options: ImpactOptions,
    /// Drop `groups` and `tests` from the answer.
    pub(crate) summary: bool,
    /// The most estimated tokens the reply may cost.
    pub(crate) max_tokens: Option<u64>,
}

/// `data` of a successful `impact`. `groups` and `tests` are absent under
/// `--summary`, and `tests` is absent under `--no-tests`.
#[derive(Clone, Serialize)]
pub(crate) struct ImpactResult {
    symbol: SymbolOut,
    risk: &'static str,
    exact: bool,
    reasons: Vec<ReasonOut>,
    counts: CountsOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    groups: Option<Vec<GroupOut>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tests: Option<Vec<TestOut>>,
    options: OptionsOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Tokens>,
}

/// Under a budget, the deepest group's hits go first, from the end, then
/// the next group's, then the tests from the end. Groups, counts, reasons
/// and risk stay.
impl Shed for ImpactResult {
    fn sheddable(&self) -> usize {
        let hits: usize = self
            .groups
            .iter()
            .flatten()
            .map(|group| group.hits.len())
            .sum();
        hits + self.tests.as_ref().map_or(0, Vec::len)
    }

    fn shed(&mut self, mut count: usize) {
        let mut groups: Vec<&mut GroupOut> = self.groups.iter_mut().flatten().collect();
        groups.sort_by_key(|group| Reverse(group.depth));
        for group in groups {
            drop_last(&mut group.hits, &mut count);
        }
        if let Some(tests) = &mut self.tests {
            drop_last(tests, &mut count);
        }
    }

    fn set_tokens(&mut self, tokens: Tokens) {
        self.tokens = Some(tokens);
    }
}

/// One completeness reason. `count` is null where it cannot be counted.
#[derive(Clone, Serialize)]
struct ReasonOut {
    reason: &'static str,
    count: Option<u64>,
}

#[derive(Clone, Serialize)]
struct CountsOut {
    direct: u64,
    total: u64,
    packages: u64,
    tests: u64,
    below_floor: u64,
}

/// The non-test hits of one depth: `total` of them, of which `hits` holds
/// at most 100, with `truncated` set when there were more.
#[derive(Clone, Serialize)]
struct GroupOut {
    depth: u8,
    total: usize,
    truncated: bool,
    hits: Vec<HitOut>,
}

/// One reached symbol. `name`, `kind`, `path`, and `line` describe it;
/// `reason` and the site are the edge it was best reached by, and
/// `confidence` is the product along that path.
#[derive(Clone, Serialize)]
struct HitOut {
    name: String,
    kind: String,
    path: String,
    line: Option<i64>,
    confidence: f64,
    reason: String,
    site_path: String,
    site_line: Option<i64>,
}

/// A reached test: the hit shape plus its depth.
#[derive(Clone, Serialize)]
struct TestOut {
    depth: u8,
    #[serde(flatten)]
    hit: HitOut,
}

#[derive(Clone, Serialize)]
struct OptionsOut {
    depth: u8,
    min_confidence: f64,
    include_tests: bool,
}

/// The impact of the one symbol `request.symbol` names in the index of
/// `request.repo`, fitted to `request.max_tokens`.
pub(crate) fn impact(request: &ImpactRequest) -> Result<Reply<ImpactResult>, Failure> {
    if !request.options.in_range() {
        return Err(Failure::new(
            ErrorKind::Usage,
            "depth must be 1 to 5 and min_confidence above 0 and at most 1",
        ));
    }
    budget::check(request.max_tokens)?;
    let (store, names) = current_index(&request.repo)?;
    let found = lookup(&store, &names, &request.symbol)?;
    let options = request.options;
    let answer = phantom_fractal_analyze::impact(&store, found.id, &options)?;
    let (groups, tests) = if request.summary {
        (None, None)
    } else {
        let groups = answer
            .groups
            .into_iter()
            .map(|group| group_out(&names, group))
            .collect();
        let tests = options.include_tests.then(|| {
            answer
                .tests
                .into_iter()
                .map(|hit| TestOut {
                    depth: hit.depth,
                    hit: hit_out(&names, hit),
                })
                .collect()
        });
        (Some(groups), tests)
    };
    let result = ImpactResult {
        symbol: symbol_out(&names, found),
        risk: answer.risk.as_str(),
        exact: answer.exact,
        reasons: answer.reasons.into_iter().map(reason_out).collect(),
        counts: counts_out(answer.counts),
        groups,
        tests,
        options: OptionsOut {
            depth: options.max_depth,
            min_confidence: options.min_confidence,
            include_tests: options.include_tests,
        },
        tokens: None,
    };
    super::answer(result, &store, &request.repo, request.max_tokens)
}

fn reason_out(reason: ReasonCount) -> ReasonOut {
    ReasonOut {
        reason: reason.reason.as_str(),
        count: reason.count,
    }
}

const fn counts_out(counts: Counts) -> CountsOut {
    CountsOut {
        direct: counts.direct,
        total: counts.total,
        packages: counts.packages,
        tests: counts.tests,
        below_floor: counts.below_floor,
    }
}

fn group_out(names: &Names, group: Group) -> GroupOut {
    GroupOut {
        depth: group.depth,
        total: group.total,
        truncated: group.truncated,
        hits: group
            .hits
            .into_iter()
            .map(|hit| hit_out(names, hit))
            .collect(),
    }
}

/// A hit with its symbol named as `callers` names it. A symbol the
/// resolver input does not hold keeps its bare stored name.
fn hit_out(names: &Names, hit: Hit) -> HitOut {
    let symbol = hit.symbol;
    HitOut {
        name: names.of(symbol.id).unwrap_or(symbol.name),
        kind: symbol.kind,
        path: symbol.file,
        line: symbol.line,
        confidence: hit.confidence,
        reason: hit.via.reason,
        site_path: hit.via.site_file,
        site_line: hit.via.site_line,
    }
}
