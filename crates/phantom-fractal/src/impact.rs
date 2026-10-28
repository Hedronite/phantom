//! `impact`: what breaks if one symbol changes, from the stored call graph.
//!
//! The lookup and its errors are those of `callers`. The walk, the
//! completeness reasons, and the risk level come from
//! `phantom-fractal-analyze`. Every path is relative to the indexed tree.

use std::path::Path;

use phantom_fractal_analyze::{Counts, Group, Hit, ImpactOptions, ReasonCount};
use serde::Serialize;

use crate::Failure;
use crate::calls::{Names, SymbolOut, current_index, lookup, symbol_out};

/// What `impact` was asked for, past the symbol and the repo.
pub(crate) struct Request {
    pub(crate) options: ImpactOptions,
    /// Drop `groups` and `tests` from the answer.
    pub(crate) summary: bool,
}

/// `data` of a successful `impact`. `groups` and `tests` are absent under
/// `--summary`, and `tests` is absent under `--no-tests`.
#[derive(Serialize)]
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
}

/// One completeness reason. `count` is null where it cannot be counted.
#[derive(Serialize)]
struct ReasonOut {
    reason: &'static str,
    count: Option<u64>,
}

#[derive(Serialize)]
struct CountsOut {
    direct: u64,
    total: u64,
    packages: u64,
    tests: u64,
    below_floor: u64,
}

/// The non-test hits of one depth: `total` of them, of which `hits` holds
/// at most 100, with `truncated` set when there were more.
#[derive(Serialize)]
struct GroupOut {
    depth: u8,
    total: usize,
    truncated: bool,
    hits: Vec<HitOut>,
}

/// One reached symbol. `name`, `kind`, `path`, and `line` describe it;
/// `reason` and the site are the edge it was best reached by, and
/// `confidence` is the product along that path.
#[derive(Serialize)]
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
#[derive(Serialize)]
struct TestOut {
    depth: u8,
    #[serde(flatten)]
    hit: HitOut,
}

#[derive(Serialize)]
struct OptionsOut {
    depth: u8,
    min_confidence: f64,
    include_tests: bool,
}

/// The impact of the one symbol `query` names in the index of `repo`.
pub(crate) fn impact(query: &str, repo: &Path, request: &Request) -> Result<ImpactResult, Failure> {
    let (store, names) = current_index(repo)?;
    let found = lookup(&store, &names, query)?;
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
    Ok(ImpactResult {
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
    })
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
