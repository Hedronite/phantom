//! Completeness reasons and the risk level (SPEC-w5 G2c).

use phantom_fractal_store::{StoreError, SymbolFacts};

use crate::{Counts, ImpactSource};

/// Why an impact answer may miss users of the target. A missing reason
/// that applies would be a false "exact"; an extra one only lowers trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletenessReason {
    /// Calls or method calls named like the target that no edge places.
    UnresolvedRefs,
    /// Call-shaped names like the target inside macro tokens.
    MacroBoundary,
    /// The target is a trait-impl method, which dynamic or generic calls
    /// can reach without an edge.
    TraitDispatch,
    /// The index pass skipped `.rs` files, or the count is unknown.
    ParseSkippedFiles,
    /// A `pub` item of a lib, which code outside the indexed tree can call.
    ExternalUsersPossible,
}

impl CompletenessReason {
    /// Every reason, in the fixed order of an answer.
    pub const ALL: [Self; 5] = [
        Self::UnresolvedRefs,
        Self::MacroBoundary,
        Self::TraitDispatch,
        Self::ParseSkippedFiles,
        Self::ExternalUsersPossible,
    ];

    /// The stable snake-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnresolvedRefs => "unresolved_refs",
            Self::MacroBoundary => "macro_boundary",
            Self::TraitDispatch => "trait_dispatch",
            Self::ParseSkippedFiles => "parse_skipped_files",
            Self::ExternalUsersPossible => "external_users_possible",
        }
    }
}

/// One reason, with its count where it can be counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReasonCount {
    pub reason: CompletenessReason,
    pub count: Option<u64>,
}

/// How much a change to the target is likely to break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
    /// More users may be unseen than seen.
    Unknown,
}

impl Risk {
    /// The stable lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
            Self::Unknown => "unknown",
        }
    }
}

/// The reasons that apply to `target`, in [`CompletenessReason::ALL`]
/// order.
pub(crate) fn reasons(
    source: &impl ImpactSource,
    target: &SymbolFacts,
) -> Result<Vec<ReasonCount>, StoreError> {
    let unresolved = source.unresolved_named(&target.name)?;
    let macro_args = source.macro_args_named(&target.name)?;
    let skipped = source.rs_skipped()?;
    let mut out = Vec::new();
    for reason in CompletenessReason::ALL {
        let applies = match reason {
            CompletenessReason::UnresolvedRefs => counted(unresolved),
            CompletenessReason::MacroBoundary => counted(macro_args),
            CompletenessReason::TraitDispatch => {
                (target.kind == "method" && target.trait_name.is_some()).then_some(None)
            }
            CompletenessReason::ParseSkippedFiles => match skipped {
                None => Some(None),
                Some(0) => None,
                Some(count) => Some(Some(count)),
            },
            CompletenessReason::ExternalUsersPossible => (target.visibility == "public"
                && target.target_kind.as_deref() == Some("lib"))
            .then_some(None),
        };
        if let Some(count) = applies {
            out.push(ReasonCount { reason, count });
        }
    }
    Ok(out)
}

/// `Some(Some(n))` for a positive count, else none.
fn counted(count: i64) -> Option<Option<u64>> {
    u64::try_from(count)
        .ok()
        .filter(|&count| count > 0)
        .map(Some)
}

/// The first level that holds, in order: `critical` at 25 or more direct
/// users; `unknown` when unresolved plus macro-boundary refs reach
/// `max(1, direct)`; `high` at 10 direct or 50 total; `medium` at 3
/// direct or 10 total; else `low`. Tests are never counted.
pub(crate) fn risk(counts: &Counts, reasons: &[ReasonCount]) -> Risk {
    let unseen: u64 = reasons
        .iter()
        .filter(|reason| {
            matches!(
                reason.reason,
                CompletenessReason::UnresolvedRefs | CompletenessReason::MacroBoundary
            )
        })
        .filter_map(|reason| reason.count)
        .sum();
    if counts.direct >= 25 {
        Risk::Critical
    } else if unseen >= counts.direct.max(1) {
        Risk::Unknown
    } else if counts.direct >= 10 || counts.total >= 50 {
        Risk::High
    } else if counts.direct >= 3 || counts.total >= 10 {
        Risk::Medium
    } else {
        Risk::Low
    }
}
