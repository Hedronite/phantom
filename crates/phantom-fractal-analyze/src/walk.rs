//! The breadth-first walk over incoming calls (SPEC-w5 G2b).

use std::collections::{BTreeMap, HashSet};

use phantom_fractal_store::{StoreError, SymbolFacts};

use crate::{Counts, Group, Hit, ImpactOptions, ImpactSource, Via};

/// The walk's groups, tests, and counts.
pub(crate) struct Walked {
    pub groups: Vec<Group>,
    pub tests: Vec<Hit>,
    pub counts: Counts,
}

/// The best way one level reached a symbol.
struct Reach {
    confidence: f64,
    via: Via,
}

/// Walks level by level from `target`, which has confidence 1.0.
///
/// Every symbol reached is placed at the level it is first reached, with
/// the best product over its edges from the previous level's expanded
/// symbols. Ties keep the first edge met: the previous level is read in
/// hit order (file, line, name) and each symbol's callers in the store's
/// order. An edge to a symbol already placed, the target included, is
/// ignored. A placed symbol is then, in this order:
///
/// - a test left out, when `include_tests` is false: nothing is counted;
/// - below the floor: counted in `below_floor`;
/// - a test: listed in `tests`;
/// - otherwise reported in its depth's group and expanded next level.
///
/// Neither a test nor a symbol below the floor is expanded.
pub(crate) fn walk(
    source: &impl ImpactSource,
    target: i64,
    options: &ImpactOptions,
) -> Result<Walked, StoreError> {
    let mut placed: HashSet<i64> = HashSet::from([target]);
    let mut frontier: Vec<(i64, f64)> = vec![(target, 1.0)];
    let mut out = Walked {
        groups: Vec::new(),
        tests: Vec::new(),
        counts: Counts::default(),
    };
    let mut packages: HashSet<String> = HashSet::new();
    for depth in 1..=options.max_depth {
        if frontier.is_empty() {
            break;
        }
        let reached = level(source, &frontier, &placed)?;
        placed.extend(reached.keys());
        let mut hits = Vec::new();
        for (id, reach) in reached {
            if let Some(symbol) = source.facts(id)? {
                let hit = Hit {
                    symbol,
                    depth,
                    confidence: reach.confidence,
                    via: reach.via,
                };
                place(hit, options, &mut out, &mut hits);
            }
        }
        sort(&mut hits);
        frontier = hits
            .iter()
            .map(|hit| (hit.symbol.id, hit.confidence))
            .collect();
        count(&hits, depth, &mut out.counts, &mut packages);
        if !hits.is_empty() {
            out.groups.push(group(depth, hits));
        }
    }
    out.tests.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| order(&a.symbol, &b.symbol))
    });
    out.counts.tests = out.tests.len() as u64;
    Ok(out)
}

/// Sends one placed symbol where the rules in [`walk`] say.
fn place(hit: Hit, options: &ImpactOptions, out: &mut Walked, hits: &mut Vec<Hit>) {
    if hit.symbol.is_test && !options.include_tests {
        return;
    }
    if hit.confidence < options.min_confidence {
        out.counts.below_floor += 1;
    } else if hit.symbol.is_test {
        out.tests.push(hit);
    } else {
        hits.push(hit);
    }
}

/// The symbols one level reaches from `frontier`, by id, each at its best
/// confidence. `frontier` is in hit order.
fn level(
    source: &impl ImpactSource,
    frontier: &[(i64, f64)],
    placed: &HashSet<i64>,
) -> Result<BTreeMap<i64, Reach>, StoreError> {
    let mut reached: BTreeMap<i64, Reach> = BTreeMap::new();
    for &(callee, confidence) in frontier {
        for hit in source.callers(callee)? {
            if placed.contains(&hit.symbol) {
                continue;
            }
            let product = confidence * hit.confidence;
            if reached
                .get(&hit.symbol)
                .is_some_and(|best| best.confidence >= product)
            {
                continue;
            }
            let via = Via {
                callee,
                reason: hit.reason,
                site_file: hit.file,
                site_line: hit.line,
            };
            reached.insert(
                hit.symbol,
                Reach {
                    confidence: product,
                    via,
                },
            );
        }
    }
    Ok(reached)
}

fn count(hits: &[Hit], depth: u8, counts: &mut Counts, packages: &mut HashSet<String>) {
    let reported = hits.len() as u64;
    if depth == 1 {
        counts.direct = reported;
    }
    counts.total += reported;
    packages.extend(hits.iter().filter_map(|hit| hit.symbol.package.clone()));
    counts.packages = packages.len() as u64;
}

fn group(depth: u8, mut hits: Vec<Hit>) -> Group {
    let total = hits.len();
    hits.truncate(Group::MAX_HITS);
    Group {
        depth,
        total,
        truncated: total > Group::MAX_HITS,
        hits,
    }
}

fn sort(hits: &mut [Hit]) {
    hits.sort_by(|a, b| order(&a.symbol, &b.symbol));
}

/// File, then line (a missing line last), then name, then id.
fn order(a: &SymbolFacts, b: &SymbolFacts) -> std::cmp::Ordering {
    a.file
        .cmp(&b.file)
        .then_with(|| line_key(a).cmp(&line_key(b)))
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.id.cmp(&b.id))
}

fn line_key(symbol: &SymbolFacts) -> (bool, i64) {
    (symbol.line.is_none(), symbol.line.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_line_sorts_after_any_line() {
        let fact = |line| SymbolFacts {
            id: 1,
            name: "f".to_owned(),
            kind: "function".to_owned(),
            file: "a.rs".to_owned(),
            line,
            visibility: "private".to_owned(),
            trait_name: None,
            is_test: false,
            package: None,
            target_kind: None,
        };
        assert!(order(&fact(Some(i64::MAX)), &fact(None)).is_lt());
    }
}
