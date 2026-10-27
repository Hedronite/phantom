//! The four-tier call resolver (SPEC-w3 G2b).
//!
//! [`resolve`] is a pure function of its input. Only `Call` references with
//! a `from` symbol that are not `shadowed` are candidates. Each candidate
//! gets exactly one edge or none:
//!
//! | Reason | Confidence | When |
//! |---|---|---|
//! | `PathExact` | 1.0 | The path starts with `crate`, `self`, or `super`. |
//! | `Local` | 0.95 | The first segment is declared in the call's module. |
//! | `UseAlias` | 0.95 | The first segment is bound by a non-glob `use`. |
//! | `Assoc` | 0.9 | `Type::name` to the one inherent method of `Type`. |
//!
//! Method calls on values, trait dispatch, glob imports, and calls across
//! crates are counted and never resolved.

mod lookup;

use phantom_fractal_lang::{Edge, Reason, RefKind, ResolveCounts, ResolveInput};

use lookup::{Index, Placed};

/// The confidence of an edge placed for `reason`.
pub fn confidence(reason: Reason) -> f64 {
    match reason {
        Reason::PathExact => 1.0,
        Reason::Local | Reason::UseAlias => 0.95,
        Reason::Assoc => 0.9,
    }
}

/// Places every call reference in `input`, in ref order.
///
/// Counts: `edges` placed; `unresolved` for a call into the workspace no
/// tier places, including a shadowed call, an ambiguous one, a name only a
/// glob could supply, and a call in an orphan file; `external` when the
/// first segment is `std`, `core`, `alloc`, or not found; `no_source` for a
/// call outside any function symbol; `method_calls` and `macros` for the
/// other reference kinds.
pub fn resolve(input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
    let index = Index::new(input);
    let mut edges = Vec::new();
    let mut counts = ResolveCounts::default();
    for call in &input.refs {
        match call.kind {
            RefKind::MethodCall => {
                counts.method_calls += 1;
                continue;
            }
            RefKind::Macro => {
                counts.macros += 1;
                continue;
            }
            RefKind::Call => {}
        }
        let Some(source) = call.from else {
            counts.no_source += 1;
            continue;
        };
        match index.place(call) {
            Placed::Edge(target, reason) => {
                counts.edges += 1;
                edges.push(Edge {
                    ref_id: call.id,
                    source,
                    target,
                    confidence: confidence(reason),
                    reason,
                });
            }
            Placed::Unresolved => counts.unresolved += 1,
            Placed::External => counts.external += 1,
        }
    }
    (edges, counts)
}

/// Path segments with generic arguments removed, or none for a path that
/// starts at a qualified type such as `<T as Trait>::f`. A leading `::`
/// gives an empty first segment.
pub(crate) fn segments(path: &str) -> Option<Vec<String>> {
    if path.starts_with('<') {
        return None;
    }
    let mut plain = String::with_capacity(path.len());
    let mut depth = 0_usize;
    for ch in path.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(ch),
            _ => {}
        }
    }
    let mut parts: Vec<String> = plain.split("::").map(str::to_owned).collect();
    let leading_empty = parts.first().is_some_and(String::is_empty);
    parts.retain(|part| !part.is_empty());
    if leading_empty {
        parts.insert(0, String::new());
    }
    (!parts.is_empty()).then_some(parts)
}
