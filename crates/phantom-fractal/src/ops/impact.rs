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
use super::distinct::{Distinct, Site};
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
/// `--summary`, and `tests` is absent under `--no-tests`. The
/// `callers_distinct`, `files` and `sites` keys are over the listed hits
/// and tests, and are absent under `--summary`.
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
    #[serde(flatten)]
    distinct: Option<Distinct>,
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

    fn drop_extras(&mut self) {
        let groups = self.groups.iter_mut().flatten();
        for hit in groups.flat_map(|group| &mut group.hits) {
            hit.display = None;
        }
        for test in self.tests.iter_mut().flatten() {
            test.hit.display = None;
        }
        self.distinct = None;
    }
}

/// One completeness reason. `count` is null where it cannot be counted.
#[derive(Clone, Serialize)]
pub(crate) struct ReasonOut {
    reason: &'static str,
    count: Option<u64>,
}

#[derive(Clone, Serialize)]
pub(crate) struct CountsOut {
    direct: u64,
    total: u64,
    packages: u64,
    tests: u64,
    below_floor: u64,
}

/// The non-test hits of one depth: `total` of them, of which `hits` holds
/// at most 100, with `truncated` set when there were more.
#[derive(Clone, Serialize)]
pub(crate) struct GroupOut {
    pub(crate) depth: u8,
    total: usize,
    truncated: bool,
    pub(crate) hits: Vec<HitOut>,
}

/// One reached symbol. `name`, `kind`, `path`, and `line` describe it;
/// `reason` and the site are the edge it was best reached by, and
/// `confidence` is the product along that path. `display` names it by
/// file, as `callers` does; `impact` sets it, and `impact --diff` does not.
#[derive(Clone, Serialize)]
pub(crate) struct HitOut {
    name: String,
    kind: String,
    path: String,
    line: Option<i64>,
    confidence: f64,
    reason: String,
    site_path: String,
    site_line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<String>,
}

/// A reached test: the hit shape plus its depth.
#[derive(Clone, Serialize)]
pub(crate) struct TestOut {
    depth: u8,
    #[serde(flatten)]
    hit: HitOut,
}

/// Whether each hit carries its `display`: `impact` names its hits so,
/// and `impact --diff` keeps its old shape.
#[derive(Clone, Copy)]
pub(crate) enum Shown {
    Display,
    Plain,
}

#[derive(Clone, Serialize)]
pub(crate) struct OptionsOut {
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
            .map(|group| group_out(&names, group, Shown::Display))
            .collect();
        let tests = options.include_tests.then(|| {
            answer
                .tests
                .into_iter()
                .map(|hit| test_out(&names, hit, Shown::Display))
                .collect()
        });
        (Some(groups), tests)
    };
    let distinct = groups.as_ref().map(|groups: &Vec<GroupOut>| {
        let hits = groups.iter().flat_map(|group| &group.hits);
        let tests = tests.iter().flatten().map(|test: &TestOut| &test.hit);
        Distinct::of(hits.chain(tests).map(HitOut::site))
    });
    let result = ImpactResult {
        symbol: symbol_out(&names, found),
        risk: answer.risk.as_str(),
        exact: answer.exact,
        reasons: answer.reasons.into_iter().map(reason_out).collect(),
        counts: counts_out(answer.counts),
        groups,
        tests,
        options: options_out(&options),
        distinct,
        tokens: None,
    };
    super::answer(result, &store, &request.repo, request.max_tokens)
}

pub(crate) fn reason_out(reason: ReasonCount) -> ReasonOut {
    ReasonOut {
        reason: reason.reason.as_str(),
        count: reason.count,
    }
}

pub(crate) const fn counts_out(counts: Counts) -> CountsOut {
    CountsOut {
        direct: counts.direct,
        total: counts.total,
        packages: counts.packages,
        tests: counts.tests,
        below_floor: counts.below_floor,
    }
}

pub(crate) fn group_out(names: &Names, group: Group, shown: Shown) -> GroupOut {
    GroupOut {
        depth: group.depth,
        total: group.total,
        truncated: group.truncated,
        hits: group
            .hits
            .into_iter()
            .map(|hit| hit_out(names, hit, shown))
            .collect(),
    }
}

/// A reached test, named as [`hit_out`] names a hit.
pub(crate) fn test_out(names: &Names, hit: Hit, shown: Shown) -> TestOut {
    TestOut {
        depth: hit.depth,
        hit: hit_out(names, hit, shown),
    }
}

/// The options an answer was walked with.
pub(crate) const fn options_out(options: &ImpactOptions) -> OptionsOut {
    OptionsOut {
        depth: options.max_depth,
        min_confidence: options.min_confidence,
        include_tests: options.include_tests,
    }
}

/// A hit with its symbol named as `callers` names it. A symbol the
/// resolver input does not hold keeps its bare stored name.
fn hit_out(names: &Names, hit: Hit, shown: Shown) -> HitOut {
    let symbol = hit.symbol;
    let display = match shown {
        Shown::Display => Some(names.display_of(symbol.id, &symbol.file, &symbol.name)),
        Shown::Plain => None,
    };
    HitOut {
        name: names.of(symbol.id).unwrap_or(symbol.name),
        kind: symbol.kind,
        path: symbol.file,
        line: symbol.line,
        confidence: hit.confidence,
        reason: hit.via.reason,
        site_path: hit.via.site_file,
        site_line: hit.via.site_line,
        display,
    }
}

impl HitOut {
    /// This hit as [`Distinct`] groups it: by the symbol reached, in its
    /// own file, at the site it was best reached by.
    fn site(&self) -> Site<'_> {
        Site {
            display: self.display.as_deref().unwrap_or(&self.name),
            file: &self.path,
            line: self.site_line,
            confidence: self.confidence,
        }
    }
}
