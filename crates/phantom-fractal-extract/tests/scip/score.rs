//! The span matcher and the deterministic report (SPEC-w10 G1, its
//! W11 G1a counts): definitions are document-scoped (F2), the
//! coverage-cast subsets per export (F1) sit beside the full figures,
//! the W10 wide recall prints as `recall_unscoped`, and every miss
//! bucket splits by the caller file's module claim. A SCIP
//! its file whose inclusive span contains the definition line and
//! whose name equals the last name in the SCIP descriptor; a
//! reference belongs to the innermost span that contains it; the
//! truth set is the distinct `(caller, callee)` pairs where both map
//! to workspace symbols; a Phantom `call` edge is corroborated when
//! its pair is in the truth set. The report is one JSON object built
//! from sorted keys and fixed-width numbers, so two runs of the same
//! tree print the same bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use phantom_fractal_store::{EdgeRow, ScoreCallRef, SymbolSpan};

use crate::scip::wire::ScipIndex;

/// The miss buckets (W12 D4): the W11 coarse method and macro buckets
/// split by the receiver shape and the hint kind. Every bucket keeps
/// its `in_module`/`orphan` split, a miss lands in exactly one, and
/// the buckets sum to the misses. The report map sorts the keys; the
/// ret and ret? hints arrive in G2a and read zero before it.
pub const MISS_BUCKETS: [&str; 14] = [
    "other",
    "trait-method callee",
    "unresolved path call",
    "macro_arg path shape",
    "macro_arg method shape",
    "macro invocation",
    "identifier with hint type",
    "identifier with hint ctor",
    "identifier with hint ctor?",
    "identifier with hint ret",
    "identifier with hint ret?",
    "identifier with no hint",
    "self.field",
    "call or chained receiver",
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

/// The full scorer, over one parsed SCIP index and one indexed
/// store.
///
/// The definition scope is per document (F2, SPEC-w11 G1a): a
/// reference takes the callees its own document mapped, and only a
/// document with no mapped definition of that symbol falls back to
/// every mapping.
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

    // Definition maps (F2): one `(symbol, document)` key per SCIP
    // definition, each mapping to the innermost named span of its
    // own document.
    let mut definitions: BTreeMap<(String, String), BTreeSet<i64>> = BTreeMap::new();
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
                    .entry((occurrence.symbol.clone(), document.relative_path.clone()))
                    .or_default()
                    .insert(span.id);
            }
        }
    }
    // The shipped fallback: a symbol whose definitions span documents.
    let mut every: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for ((symbol, _document), callees) in &definitions {
        every
            .entry(symbol.clone())
            .or_default()
            .extend(callees.iter().copied());
    }

    // The truth set: a reference's caller is the span around it; its
    // callees are whatever the referenced symbol mapped to. The W10
    // fan-out truth (`truth_wide`) keeps the fallback pair the
    // document scope narrows away, for `recall_unscoped`.
    let mut truth: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut truth_wide: BTreeSet<(i64, i64)> = BTreeSet::new();
    for document in &index.documents {
        for occurrence in &document.occurrences {
            if occurrence.is_definition {
                continue;
            }
            let line = occurrence.range.start_line + 1;
            let Some(caller) = innermost(&spans, &document.relative_path, line, None) else {
                continue;
            };
            // A reference takes the callees its own document defined;
            // one whose document carried none takes every mapped
            // callee, the shipped behaviour.
            let empty = BTreeSet::new();
            let local = definitions
                .get(&(occurrence.symbol.clone(), document.relative_path.clone()))
                .unwrap_or(&empty);
            let callees = if local.is_empty() {
                every.get(&occurrence.symbol).unwrap_or(&empty)
            } else {
                local
            };
            for callee in callees {
                truth.insert((caller.id, *callee));
            }
            // The W10 fan-out: the document claim dropped, every mapped
            // callee stays a truth pair — the `recall_unscoped` set.
            for callee in every
                .get(&occurrence.symbol)
                .map(|set| set.iter().collect::<Vec<_>>())
                .unwrap_or_default()
            {
                truth_wide.insert((caller.id, *callee));
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

    // The cover set (F1): the documents the export carries. A pair
    // is covered when both its span files sit in that set.
    let exported: BTreeSet<&str> = index
        .documents
        .iter()
        .map(|document| document.relative_path.as_str())
        .collect();
    let covered_pair = |pair: (i64, i64)| {
        matches!(
            (by_id.get(&pair.0), by_id.get(&pair.1)),
            (Some(a), Some(b))
                if exported.contains(a.path.as_str())
                    && exported.contains(b.path.as_str())
        )
    };

    // Per-reason and per-band edge-carry counts, full and covered. The
    // covered counts are kept per reason and per band, as F1 asks.
    let mut reasons: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut bands: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut covered: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut covered_bands: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for edge in &edges {
        let is_covered = covered_pair((edge.src_id, edge.dst_id));
        pair_into(&mut reasons, edge.reason.clone(), &truth, edge);
        if is_covered {
            pair_into(&mut covered, edge.reason.clone(), &truth, edge);
        }
        for band in bands_of(edge.confidence) {
            pair_into(&mut bands, band.to_owned(), &truth, edge);
            if is_covered {
                pair_into(&mut covered_bands, band.to_owned(), &truth, edge);
            }
        }
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

    // The miss buckets over the truth set minus the hits, each with
    // the caller's file-module split (`in_module`, `orphan`).
    let mut misses: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for bucket in MISS_BUCKETS {
        misses.insert(bucket, (0, 0));
    }
    for pair in truth.difference(&hits) {
        let Some(caller) = by_id.get(&pair.0) else {
            misses.get_mut("other").expect("named bucket").0 += 1;
            continue;
        };
        let Some(callee) = by_id.get(&pair.1) else {
            misses.get_mut("other").expect("named bucket").0 += 1;
            continue;
        };
        let in_module = store
            .file_has_module(caller.file_id)
            .map_err(|err| format!("store: {err}"))?;
        let callee_name = callee.name.clone();
        let is_trait = store
            .symbol_facts(pair.1)
            .map_err(|err| format!("store: {err}"))?
            .map(|facts| facts.trait_name.is_some())
            .unwrap_or(false);
        let refs = store
            .score_call_refs(pair.0)
            .map_err(|err| format!("store: {err}"))?;
        let bucket = miss_bucket(&refs, callee_name.as_str(), is_trait);
        let slot = misses.get_mut(bucket).expect("named bucket");
        if in_module {
            slot.0 += 1;
        } else {
            slot.1 += 1;
        }
    }

    // The origin line (W12 D4): the edges whose source ref is a
    // macro_arg ref, with the same corroborated reading as the bands.
    let macro_arg_refs = store
        .ref_ids_of_kind("macro_arg")
        .map_err(|err| format!("store: {err}"))?;
    let mut origin = (0usize, 0usize);
    for edge in &edges {
        if edge
            .ref_id
            .is_some_and(|ref_id| macro_arg_refs.contains(&ref_id))
        {
            origin.0 += 1;
            if truth.contains(&(edge.src_id, edge.dst_id)) {
                origin.1 += 1;
            }
        }
    }

    // The report.
    let mut out = String::from("{\"reasons\":{");
    for (position, (reason, count)) in reasons.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        let carried = covered.get(reason).copied().unwrap_or((0, 0));
        out.push_str(&json(reason));
        out.push(':');
        out.push_str(&triple_full(count, &carried));
    }
    out.push_str("},\"confidence\":{");
    for (position, (band, count)) in bands.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        let carried = covered_bands.get(band).copied().unwrap_or((0, 0));
        out.push_str(&json(band));
        out.push(':');
        out.push_str(&triple_full(count, &carried));
    }
    out.push_str("},\"recall\":");
    out.push_str(&recall(&truth, &hits, &cross, &test_callers, &covered_pair));
    out.push_str(",\"recall_unscoped\":");
    out.push_str(&split(
        truth_wide.len(),
        hits.intersection(&truth_wide).count(),
    ));
    out.push_str(",\"misses\":{");
    for (position, (bucket, count)) in misses.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        out.push_str(&json(bucket));
        out.push(':');
        out.push_str(&split_pair(count));
    }
    out.push_str("},\"from_macro_arg\":");
    out.push_str(&triple(&origin));
    out.push('}');
    Ok(out)
}

/// One group's `{edges, corroborated, precision, covered}` — the
/// full denominator first, the coverage-cast subset after (F1).
fn triple_full(count: &(usize, usize), covered: &(usize, usize)) -> String {
    format!(
        "{{\"edges\":{},\"corroborated\":{},\"precision\":{},\"covered\":{}}}",
        count.0,
        count.1,
        ratio(count.1, count.0),
        triple(covered)
    )
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
    covered_pair: &dyn Fn((i64, i64)) -> bool,
) -> String {
    // The scoped numbers (F1), then the same splits measured in
    // scope.
    let scoped_truth: BTreeSet<(i64, i64)> = truth
        .iter()
        .copied()
        .filter(|pair| covered_pair(*pair))
        .collect();
    let scoped_hits: BTreeSet<(i64, i64)> = hits
        .intersection(truth)
        .copied()
        .filter(|pair| covered_pair(*pair))
        .collect();
    let total = scoped_truth.len();
    let total_hits = scoped_hits.len();
    let cross_hits = scoped_truth
        .intersection(cross)
        .filter(|pair| scoped_hits.contains(*pair))
        .count();
    let test_hits = scoped_truth
        .intersection(test)
        .filter(|pair| scoped_hits.contains(*pair))
        .count();
    let test_truth = scoped_truth.intersection(test).count();
    let production_truth = total - test_truth;
    let production_hits = total_hits - test_hits;
    format!(
        "{{\"truth\":{},\"hits\":{},\"ratio\":{},\
         \"cross_file\":{},\
         \"production_callers\":{},\
         \"test_callers\":{}}}",
        total,
        total_hits,
        ratio(total_hits, total),
        split(cross.len(), cross_hits),
        split(production_truth, production_hits),
        split(test_truth, test_hits),
    )
}

/// One bucket's `{in_module, orphan}` counters, the split the
/// caller's file gives.
fn split_pair(count: &(usize, usize)) -> String {
    format!("{{\"in_module\":{},\"orphan\":{}}}", count.0, count.1)
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
pub(crate) fn miss_bucket(
    refs: &[ScoreCallRef],
    callee_name: &str,
    is_trait: bool,
) -> &'static str {
    let candidates: Vec<&ScoreCallRef> = refs
        .iter()
        .filter(|reference| last_segment_text(&reference.text) == callee_name)
        .collect();
    if candidates.is_empty() {
        return "other";
    }
    // The macro-argument shapes first: a call inside a macro argument
    // is the more specific signal than the invocation that carries it,
    // and a macro_arg ref with a receiver is the method shape, without
    // one the path shape.
    let macro_args: Vec<&ScoreCallRef> = candidates
        .iter()
        .copied()
        .filter(|reference| reference.kind == "macro_arg")
        .collect();
    if macro_args
        .iter()
        .any(|reference| reference.receiver.is_some())
    {
        return "macro_arg method shape";
    }
    if !macro_args.is_empty() {
        return "macro_arg path shape";
    }
    if candidates.iter().any(|reference| reference.kind == "macro") {
        return "macro invocation";
    }
    if candidates
        .iter()
        .any(|reference| reference.kind == "call" && !reference.placed)
    {
        return "unresolved path call";
    }
    let methods: Vec<&ScoreCallRef> = candidates
        .iter()
        .copied()
        .filter(|reference| reference.kind == "method_call")
        .collect();
    if !methods.is_empty() {
        if is_trait {
            return "trait-method callee";
        }
        return method_shape(&methods);
    }
    "other"
}

/// The receiver-shape split of the unplaced method-call buckets (W12
/// D4). The hint kind wins first - the old hinted-but-not-placed
/// split - then the unhinted identifier, `self.field`, the call or
/// chained receiver, and the catch-all. The first bucket any ref
/// matches in that order is the answer, so a caller with several
/// same-named calls buckets deterministically.
fn method_shape(methods: &[&ScoreCallRef]) -> &'static str {
    let kinds: Vec<Option<&str>> = methods
        .iter()
        .map(|reference| {
            reference
                .receiver_type
                .as_deref()
                .map(|hint| hint.split(':').next().unwrap_or(""))
        })
        .collect();
    for (kind, bucket) in [
        ("type", "identifier with hint type"),
        ("ctor", "identifier with hint ctor"),
        ("ctor?", "identifier with hint ctor?"),
        ("ret", "identifier with hint ret"),
        ("ret?", "identifier with hint ret?"),
    ] {
        if kinds.contains(&Some(kind)) {
            return bucket;
        }
    }
    if kinds.iter().any(Option::is_some) {
        // A hint kind outside the five is its own caller shape; the
        // catch-all holds it until a bucket of its own earns a case.
        return "other";
    }
    if methods.iter().any(|reference| {
        reference
            .receiver
            .as_deref()
            .is_some_and(|text| text.chars().all(|c| c.is_alphanumeric() || c == '_'))
    }) {
        return "identifier with no hint";
    }
    if methods.iter().any(|reference| is_field_of_self(reference)) {
        return "self.field";
    }
    if methods.iter().any(|reference| {
        reference
            .receiver
            .as_deref()
            .is_some_and(|text| text.contains('(') || text.contains('.'))
    }) {
        return "call or chained receiver";
    }
    "other"
}

/// True when the receiver expression starts at `self` and reads a
/// field of it: `self.f`, not a bare `self` and not a call on one.
fn is_field_of_self(reference: &ScoreCallRef) -> bool {
    reference
        .receiver
        .as_deref()
        .is_some_and(|text| text.starts_with("self.") && !text.contains('('))
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

/// The confidence bands an edge counts in: `0.85` and `0.5` at or above
/// those confidences, and `all`.
fn bands_of(confidence: f64) -> impl Iterator<Item = &'static str> {
    [
        ("0.85", confidence >= 0.85),
        ("0.5", confidence >= 0.5),
        ("all", true),
    ]
    .into_iter()
    .filter_map(|(band, counts)| counts.then_some(band))
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
