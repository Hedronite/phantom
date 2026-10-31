//! The span matcher and the deterministic report (SPEC-w10 G1): a SCIP
//! definition maps to the innermost Phantom `fn` or method symbol in
//! its file whose inclusive span contains the definition line and
//! whose name equals the last name in the SCIP descriptor; a
//! reference belongs to the innermost span that contains it; the
//! truth set is the distinct `(caller, callee)` pairs where both map
//! to workspace symbols; a Phantom `call` edge is corroborated when
//! its pair is in the truth set. The report is one JSON object built
//! from sorted keys and fixed-width numbers, so two runs of the same
//! tree print the same bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use phantom_fractal_store::{EdgeRow, SymbolCallRef, SymbolSpan};

use crate::scip::wire::ScipIndex;

/// The miss buckets, in the order the report prints them.
pub const MISS_BUCKETS: [&str; 6] = [
    "non-self method call with no hint",
    "hinted but not placed",
    "unresolved path call",
    "macro argument",
    "trait-method callee",
    "other",
];

/// The last name in a SCIP symbol string's descriptors. The tail past
/// the package's own space-separated parts carries the descriptor
/// chain; the separator set splits each name from the next chain
/// part, so the last non-empty token is the trailing descriptor's
/// name, and a `local` symbol — scoped to one document — never maps.
pub fn last_descriptor(symbol: &str) -> Option<&str> {
    if symbol.starts_with("local ") {
        return None;
    }
    // The method name sits before its disambiguator paren.
    let before_paren = symbol.rsplit(' ').next()?.split('(').next()?;
    tsv_last(before_paren.split(['/', '#', ':', '!', '.', '[', ']']))
}

/// The innermost containing span: the narrowest hit, ties to the
/// deepest start. SCIP's 0-based line reads in the Phantom 1-based
/// space, which is what the extractor's span lines carry.
fn innermost<'a>(
    spans: &'a [SymbolSpan],
    path: &str,
    line: i64,
    name: Option<&str>,
) -> Option<&'a SymbolSpan> {
    spans
        .iter()
        .filter(|span| span.path == path)
        .filter(|span| span.start_line <= line && line <= span.end_line)
        .filter(|span| match name {
            Some(name) => span.name == name,
            None => true,
        })
        .min_by_key(|span| {
            (
                span.end_line - span.start_line,
                std::cmp::Reverse(span.start_line),
            )
        })
}

/// The full scorer, over one parsed SCIP index and one indexed store.
pub fn score(index: &ScipIndex, store: &phantom_fractal_store::Store) -> Result<String, String> {
    let all = store
        .symbol_spans()
        .map_err(|err| format!("store: {err}"))?;
    let spans: Vec<SymbolSpan> = all
        .into_iter()
        .filter(|span| span.kind == "function" || span.kind == "method")
        .collect();
    let by_id: HashMap<i64, SymbolSpan> =
        spans.iter().map(|span| (span.id, span.clone())).collect();

    // Definition maps: one SCIP symbol that carries a Definition
    // occurrence maps to the innermost named span of its document.
    let mut definitions: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for document in &index.documents {
        for occurrence in &document.occurrences {
            if !occurrence.is_definition {
                continue;
            }
            let line = occurrence.range.start_line + 1;
            let Some(name) = last_descriptor(&occurrence.symbol) else {
                continue;
            };
            if let Some(span) = innermost(&spans, &document.relative_path, line, Some(name)) {
                definitions
                    .entry(occurrence.symbol.clone())
                    .or_default()
                    .insert(span.id);
            }
        }
    }

    // The truth set: a reference's caller is the span around it; its
    // callees are whatever the referenced symbol mapped to.
    let mut truth: BTreeSet<(i64, i64)> = BTreeSet::new();
    for document in &index.documents {
        for occurrence in &document.occurrences {
            if occurrence.is_definition {
                continue;
            }
            let line = occurrence.range.start_line + 1;
            let Some(caller) = innermost(&spans, &document.relative_path, line, None) else {
                continue;
            };
            if let Some(callees) = definitions.get(&occurrence.symbol) {
                for callee in callees {
                    truth.insert((caller.id, *callee));
                }
            }
        }
    }

    // Phantom `call` edges: the pair carry is the hit.
    let edges = store
        .edges_of_kind("call")
        .map_err(|err| format!("store: {err}"))?;
    let hits: BTreeSet<(i64, i64)> = edges
        .iter()
        .map(|edge| (edge.src_id, edge.dst_id))
        .collect();

    // Per-reason and per-band edge-carry counts.
    let mut reasons: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut bands: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for edge in &edges {
        pair_into(&mut reasons, edge.reason.clone(), &truth, edge);
        if edge.confidence >= 0.85 {
            pair_into(&mut bands, "0.85".to_owned(), &truth, edge);
        }
        if edge.confidence >= 0.5 {
            pair_into(&mut bands, "0.5".to_owned(), &truth, edge);
        }
        pair_into(&mut bands, "all".to_owned(), &truth, edge);
    }

    // The recall splits: cross-file pairs, and callers by their test
    // mark.
    let cross: BTreeSet<(i64, i64)> = truth
        .iter()
        .filter(|(caller, callee)| {
            matches!(
                (by_id.get(caller), by_id.get(callee)),
                (Some(a), Some(b)) if a.path != b.path
            )
        })
        .copied()
        .collect();
    let test_callers: BTreeSet<(i64, i64)> = truth
        .iter()
        .filter(|(caller, _)| by_id.get(caller).map(|span| span.is_test).unwrap_or(false))
        .copied()
        .collect();

    // The miss buckets over the truth set minus the hits.
    let mut misses: BTreeMap<&str, usize> = BTreeMap::new();
    for bucket in MISS_BUCKETS {
        misses.insert(bucket, 0);
    }
    for pair in truth.difference(&hits) {
        let Some(callee) = by_id.get(&pair.1) else {
            *misses.get_mut("other").expect("named bucket") += 1;
            continue;
        };
        let callee_name = callee.name.clone();
        let is_trait = store
            .symbol_facts(pair.1)
            .map_err(|err| format!("store: {err}"))?
            .map(|facts| facts.trait_name.is_some())
            .unwrap_or(false);
        let bucket = miss_bucket(store, *pair, callee_name.as_str(), is_trait)?;
        *misses.get_mut(bucket).expect("named bucket") += 1;
    }

    // The report.
    let mut out = String::from("{\"reasons\":{");
    for (position, (reason, count)) in reasons.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        out.push_str(&json(reason));
        out.push(':');
        out.push_str(&triple(count));
    }
    out.push_str("},\"confidence\":{");
    for (position, (band, count)) in bands.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        out.push_str(&json(band));
        out.push(':');
        out.push_str(&triple(count));
    }
    out.push_str("},\"recall\":");
    out.push_str(&recall(&truth, &hits, &cross, &test_callers));
    out.push_str(",\"misses\":{");
    for (position, (bucket, count)) in misses.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        out.push_str(&json(bucket));
        out.push(':');
        out.push_str(&count.to_string());
    }
    out.push_str("}}");
    Ok(out)
}

/// One group's `{edges, corroborated, precision}`.
fn triple(count: &(usize, usize)) -> String {
    format!(
        "{{\"edges\":{},\"corroborated\":{},\"precision\":{}}}",
        count.0,
        count.1,
        ratio(count.1, count.0)
    )
}

/// The recall numbers: total, cross-file only, and caller-shape
/// splits, each with truth, hits, and the ratio.
fn recall(
    truth: &BTreeSet<(i64, i64)>,
    hits: &BTreeSet<(i64, i64)>,
    cross: &BTreeSet<(i64, i64)>,
    test: &BTreeSet<(i64, i64)>,
) -> String {
    let total_hits = hits.intersection(truth).count();
    let cross_hits = cross.iter().filter(|pair| hits.contains(*pair)).count();
    let test_hits = test.iter().filter(|pair| hits.contains(*pair)).count();
    let test_truth = test.len();
    let production_truth = truth.len() - test_truth;
    let production_hits = total_hits - test_hits;
    format!(
        "{{\"truth\":{},\"hits\":{},\"ratio\":{},\
         \"cross_file\":{},\
         \"production_callers\":{},\
         \"test_callers\":{}}}",
        truth.len(),
        total_hits,
        ratio(total_hits, truth.len()),
        split(cross.len(), cross_hits),
        split(production_truth, production_hits),
        split(test_truth, test_hits),
    )
}

/// One split's `{truth, hits, ratio}`.
fn split(truth: usize, hits: usize) -> String {
    format!(
        "{{\"truth\":{},\"hits\":{},\"ratio\":{}}}",
        truth,
        hits,
        ratio(hits, truth)
    )
}

/// One missed pair's bucket, read from the caller's own call refs;
/// the callee's name and trait mark are the caller's whole context.
fn miss_bucket(
    store: &phantom_fractal_store::Store,
    pair: (i64, i64),
    callee_name: &str,
    is_trait: bool,
) -> Result<&'static str, String> {
    let refs: Vec<SymbolCallRef> = store
        .symbol_call_refs(pair.0)
        .map_err(|err| format!("store: {err}"))?;
    let candidates: Vec<&SymbolCallRef> = refs
        .iter()
        .filter(|reference| last_segment_text(&reference.text) == callee_name)
        .collect();
    if candidates.is_empty() {
        return Ok("other");
    }
    if candidates
        .iter()
        .any(|reference| reference.kind == "macro_arg" || reference.kind == "macro")
    {
        return Ok("macro argument");
    }
    if candidates
        .iter()
        .any(|reference| reference.kind == "call" && !reference.placed)
    {
        return Ok("unresolved path call");
    }
    let methods: Vec<&SymbolCallRef> = candidates
        .iter()
        .filter(|reference| reference.kind == "method_call")
        .copied()
        .collect();
    if !methods.is_empty() {
        if is_trait {
            return Ok("trait-method callee");
        }
        if methods.iter().any(|reference| reference.receiver.is_some()) {
            return Ok("hinted but not placed");
        }
        if methods.iter().any(|reference| reference.receiver.is_none()) {
            return Ok("non-self method call with no hint");
        }
    }
    Ok("other")
}

/// The last `::` segment of a ref text, so a callee is named as a call
/// writes it.
fn last_segment_text(text: &str) -> &str {
    text.rsplit("::").next().unwrap_or(text)
}

/// The ratio as one fixed-width decimal; a zero denominator is zero.
fn ratio(count: usize, denom: usize) -> String {
    if denom == 0 {
        format!("{:.4}", 0.0)
    } else {
        format!("{:.4}", count as f64 / denom as f64)
    }
}

/// One JSON string, quoted, with a backslash, a quote, and control
/// characters escaped.
/// The last non-empty split, taken from the right without the
/// double-ended dance the lint fusses over.
fn tsv_last<'a>(parts: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    parts.filter(|part| !part.is_empty()).last()
}

fn json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One group's `{edges, corroborated}` pair, keyed by text.
fn pair_into(
    groups: &mut BTreeMap<String, (usize, usize)>,
    key: String,
    truth: &BTreeSet<(i64, i64)>,
    edge: &EdgeRow,
) {
    let count = groups.entry(key).or_insert((0, 0));
    count.0 += 1;
    if truth.contains(&(edge.src_id, edge.dst_id)) {
        count.1 += 1;
    }
}
