//! The call resolver (SPEC-w3 G2b, SPEC-w4 G1, G2b), and the `import`
//! and `impl` links that reuse its path tiers (SPEC-w7 G0a).
//!
//! [`resolve`] is a pure function of its input. The candidates are `Call`
//! references with a `from` symbol that are not `shadowed`, and method
//! calls whose receiver is exactly `self`. Each candidate gets exactly one
//! edge or none:
//!
//! | Reason | Confidence | When |
//! |---|---|---|
//! | `PathExact` | 1.0 | The path starts with `crate`, `self`, or `super`. |
//! | `Local` | 0.95 | The first segment is declared in the call's module. |
//! | `UseAlias` | 0.95 | The first segment is bound by a non-glob `use`. |
//! | `Assoc` | 0.9 | `Type::name` to the one inherent method of `Type`. |
//! | `SelfMethod` | 0.9 | `self.m()` in an impl method to the one `m` of its self type. |
//! | `SelfAssoc` | 0.9 | `Self::m()` in an impl method to the one `m` of its self type. |
//! | `CratePath` | 0.95 | The path enters a workspace lib the caller may call by crate name. |
//! | `TypedMethod` | 0.8 | `x.m()` whose receiver hint names one workspace type, to its one `m`. |
//! | `UniqueMethod` | 0.5 | `x.m()` with no hint, to the one workspace method `m`, unless `m` is a common standard name. |
//!
//! A method call on a receiver other than `self` is placed through its
//! receiver hint (SPEC-w10 G2a), or, with no hint, by its name alone
//! (SPEC-w10 G3a); one that neither places is counted. Trait dispatch, glob imports, and calls into
//! crates outside the workspace are never resolved.

mod lookup;

use std::collections::HashSet;

use phantom_fractal_lang::{
    Edge, Graph, Id, Link, LinkKind, LinkReason, Node, Reason, RefKind, ResolveCounts, ResolveInput,
};

use lookup::{Index, Placed};

/// The confidence of an edge placed for `reason`.
pub fn confidence(reason: Reason) -> f64 {
    match reason {
        Reason::PathExact => 1.0,
        Reason::Local | Reason::UseAlias | Reason::CratePath => 0.95,
        Reason::Assoc | Reason::SelfMethod | Reason::SelfAssoc => 0.9,
        Reason::TypedMethod => 0.8,
        Reason::UniqueMethod => 0.5,
    }
}

/// Places every call reference in `input`, in ref order.
///
/// Counts: `edges` placed; `unresolved` for a call into the workspace no
/// tier places, including a shadowed call, an ambiguous one, a name only a
/// glob could supply, a call in an orphan file, and a `self.m()` with no
/// edge; `external` when the first segment is `std`, `core`, `alloc`, or not
/// found, including a workspace lib the caller may not call; `no_source` for a `Call` outside any function symbol;
/// `method_calls` for method calls on any receiver but `self` that got no
/// edge, and `macros`.
/// A [`RefKind::MacroArg`] gets no edge and no count.
pub fn resolve(input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
    let index = Index::new(input);
    let mut edges = Vec::new();
    let mut counts = ResolveCounts::default();
    for call in &input.refs {
        let (source, placed) = match call.kind {
            RefKind::MethodCall if call.receiver.as_deref() != Some("self") => {
                let placed = call.from.and_then(|source| {
                    let placed = match call.receiver_type.as_deref() {
                        Some(hint) => (index.place_typed(call, hint)?, Reason::TypedMethod),
                        None => (index.place_unique(call)?, Reason::UniqueMethod),
                    };
                    Some((source, placed))
                });
                match placed {
                    Some((source, (target, reason))) => (source, Placed::Edge(target, reason)),
                    None => {
                        counts.method_calls += 1;
                        continue;
                    }
                }
            }
            RefKind::MethodCall => {
                let name = segments(&call.text).and_then(|segs| segs.last().cloned());
                let (Some(source), Some(name)) = (call.from, name) else {
                    counts.unresolved += 1;
                    continue;
                };
                (source, index.place_self(source, &name, Reason::SelfMethod))
            }
            RefKind::Macro => {
                counts.macros += 1;
                continue;
            }
            // A name inside a macro's tokens: never placed, never counted.
            RefKind::MacroArg => continue,
            RefKind::Call => {
                let Some(source) = call.from else {
                    counts.no_source += 1;
                    continue;
                };
                (source, index.place(call, source))
            }
        };
        match placed {
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

/// Each method in `input` with the struct or enum that owns it, as
/// `(method, type)` in symbol order (SPEC-w6 G0b).
///
/// The owner match is the one `self-method` and `self-assoc` edges use: the
/// method's impl self type resolves to exactly one struct or enum, declared
/// in the method's own target. A method whose self type resolves to no
/// type, to more than one, to a trait, or to a type in another target gets
/// no pair. Like [`resolve`], this is a pure function of its input.
pub fn method_owners(input: &ResolveInput) -> Vec<(Id, Id)> {
    let index = Index::new(input);
    input
        .symbols
        .iter()
        .filter_map(|symbol| Some((symbol.id, index.owner_of(symbol)?)))
        .collect()
}

/// Every call edge and count, exactly as [`resolve`] gives them, and the
/// [`links`] beside them.
pub fn resolve_graph(input: &ResolveInput) -> Graph {
    let (calls, counts) = resolve(input);
    Graph {
        calls,
        counts,
        links: links(input),
    }
}

/// The `import` and `impl` links of `input` (SPEC-w7 G0a): one `import`
/// per non-glob `use` leaf whose path names exactly one workspace module
/// or symbol, in `use` order, then one `impl` per `(type, trait)` pair, in
/// the order of the first method that names it.
///
/// Both resolve their path by the call tiers and carry that tier's
/// confidence. An `import` carries the tier's reason; an `impl` carries
/// [`LinkReason::ImplTrait`]. The type of an `impl` is its methods' owner
/// by [`method_owners`], so an impl without methods gives no link. A pair
/// two impls share keeps the first one's confidence. Like [`resolve`], this
/// is a pure function of its input.
pub fn links(input: &ResolveInput) -> Vec<Link> {
    let index = Index::new(input);
    let mut out = Vec::new();
    for binding in &input.uses {
        if let Some((module, target, tier)) = index.import(binding) {
            out.push(Link {
                kind: LinkKind::Import,
                source: Node::Module(module),
                target,
                confidence: confidence(tier.reason()),
                reason: tier.link_reason(),
                ref_id: Some(binding.id),
            });
        }
    }
    let mut seen = HashSet::new();
    for method in &input.symbols {
        let Some((ty, trait_id, tier)) = index.implemented(method) else {
            continue;
        };
        if seen.insert((ty, trait_id)) {
            out.push(Link {
                kind: LinkKind::Impl,
                source: Node::Symbol(ty),
                target: Node::Symbol(trait_id),
                confidence: confidence(tier.reason()),
                reason: LinkReason::ImplTrait,
                ref_id: None,
            });
        }
    }
    out
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
