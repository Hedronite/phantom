//! Completeness reasons and the risk level (SPEC-w5 G2c).

use std::collections::BTreeSet;

use phantom_fractal_store::{StoreError, SymbolFacts};

use crate::{Counts, DiffState, ImpactSource};

/// Why an impact answer may miss users of the target. A missing reason
/// that applies would be a false "exact"; an extra one only lowers trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletenessReason {
    /// Calls or method calls named like the target that no edge places.
    UnresolvedRefs,
    /// Call-shaped names like the target inside macro tokens that no edge
    /// places (SPEC-w12 G1b). A placed one is an ordinary caller.
    MacroBoundary,
    /// The target is a trait-impl method, which dynamic or generic calls
    /// can reach without an edge.
    TraitDispatch,
    /// The index pass skipped `.rs` files, or the count is unknown.
    ParseSkippedFiles,
    /// A `pub` item of a lib, which code outside the indexed tree can call.
    ExternalUsersPossible,
    /// The index is stale against the tree (SPEC-w9 G2a): what it holds
    /// may not be what the change is walked over. [`crate::impact_many`]
    /// only.
    StaleIndex,
    /// Changed symbols the index does not hold, so no walk starts from
    /// them, counted. [`crate::impact_many`] only.
    UnindexedChanges,
    /// Target is a Go method reachable through structural interface satisfaction.
    InterfaceDispatch,
    /// Target name is referenced in a non-call value position (Go callback/func value).
    ValueReference,
    /// Source files were excluded by build constraints/tags.
    BuildExcluded,
    /// A workspace manifest changed without indexed symbol edits to walk.
    ManifestChanged,
}

impl CompletenessReason {
    /// Every reason, in the fixed order of an answer.
    pub const ALL: [Self; 11] = [
        Self::UnresolvedRefs,
        Self::MacroBoundary,
        Self::TraitDispatch,
        Self::ParseSkippedFiles,
        Self::ExternalUsersPossible,
        Self::StaleIndex,
        Self::UnindexedChanges,
        Self::InterfaceDispatch,
        Self::ValueReference,
        Self::BuildExcluded,
        Self::ManifestChanged,
    ];

    const TEXTS: &[(Self, &str)] = &[
        (Self::UnresolvedRefs, "unresolved_refs"),
        (Self::MacroBoundary, "macro_boundary"),
        (Self::TraitDispatch, "trait_dispatch"),
        (Self::ParseSkippedFiles, "parse_skipped_files"),
        (Self::ExternalUsersPossible, "external_users_possible"),
        (Self::StaleIndex, "stale_index"),
        (Self::UnindexedChanges, "unindexed_changes"),
        (Self::InterfaceDispatch, "interface_dispatch"),
        (Self::ValueReference, "value_reference"),
        (Self::BuildExcluded, "build_excluded"),
        (Self::ManifestChanged, "manifest_changed"),
    ];

    /// The stable snake-case name.
    pub fn as_str(self) -> &'static str {
        Self::TEXTS
            .iter()
            .find(|(reason, _)| *reason == self)
            .expect("all CompletenessReason variants covered")
            .1
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
/// order. The two diff reasons never apply to one target.
pub(crate) fn reasons(
    source: &impl ImpactSource,
    target: &SymbolFacts,
) -> Result<Vec<ReasonCount>, StoreError> {
    reasons_of(source, std::slice::from_ref(target), &DiffState::default())
}

/// The reasons that apply to walking from all of `targets` at once, in
/// [`CompletenessReason::ALL`] order. Unresolved and macro-boundary refs
/// are summed over the targets' distinct names; trait dispatch and
/// external users apply when they apply to any target.
struct ReasonInputs {
    unresolved: i64,
    macro_args: i64,
    value_refs: i64,
    skipped: Option<u64>,
    build_excluded: Option<u64>,
    trait_dispatch: bool,
    interface_dispatch: bool,
    external_users: bool,
}

pub(crate) fn reasons_of(
    source: &impl ImpactSource,
    targets: &[SymbolFacts],
    diff: &DiffState,
) -> Result<Vec<ReasonCount>, StoreError> {
    let inputs = collect_reason_inputs(source, targets)?;
    let mut out = Vec::new();
    for reason in CompletenessReason::ALL {
        if let Some(count) = eval_reason(reason, &inputs, diff) {
            out.push(ReasonCount { reason, count });
        }
    }
    Ok(out)
}

fn collect_reason_inputs(
    source: &impl ImpactSource,
    targets: &[SymbolFacts],
) -> Result<ReasonInputs, StoreError> {
    let names: BTreeSet<&str> = targets.iter().map(|target| target.name.as_str()).collect();
    let (unresolved, macro_args, value_refs) = sum_name_counts(source, &names)?;
    Ok(ReasonInputs {
        unresolved,
        macro_args,
        value_refs,
        skipped: source.rs_skipped()?,
        build_excluded: source.build_excluded_count()?,
        trait_dispatch: has_trait_dispatch(targets),
        interface_dispatch: has_go_method(source, targets)?,
        external_users: has_external_users(source, targets)?,
    })
}

fn sum_name_counts(
    source: &impl ImpactSource,
    names: &BTreeSet<&str>,
) -> Result<(i64, i64, i64), StoreError> {
    let mut unresolved = 0;
    let mut macro_args = 0;
    let mut value_refs = 0;
    for name in names {
        unresolved += source.unresolved_named(name)?;
        macro_args += source.macro_args_named(name)?;
        value_refs += source.value_refs_named(name)?;
    }
    Ok((unresolved, macro_args, value_refs))
}

fn eval_reason(
    reason: CompletenessReason,
    inputs: &ReasonInputs,
    diff: &DiffState,
) -> Option<Option<u64>> {
    eval_call_reason(reason, inputs)
        .or_else(|| eval_target_reason(reason, inputs))
        .or_else(|| eval_diff_reason(reason, diff))
        .flatten()
}

fn eval_call_reason(
    reason: CompletenessReason,
    inputs: &ReasonInputs,
) -> Option<Option<Option<u64>>> {
    Some(match reason {
        CompletenessReason::UnresolvedRefs => counted(inputs.unresolved),
        CompletenessReason::MacroBoundary => counted(inputs.macro_args),
        CompletenessReason::ValueReference => counted(inputs.value_refs),
        CompletenessReason::TraitDispatch => inputs.trait_dispatch.then_some(None),
        _ => return None,
    })
}

fn eval_target_reason(
    reason: CompletenessReason,
    inputs: &ReasonInputs,
) -> Option<Option<Option<u64>>> {
    Some(match reason {
        CompletenessReason::InterfaceDispatch => inputs.interface_dispatch.then_some(None),
        CompletenessReason::ExternalUsersPossible => inputs.external_users.then_some(None),
        CompletenessReason::ParseSkippedFiles => skipped_count(inputs.skipped),
        CompletenessReason::BuildExcluded => inputs.build_excluded.filter(|&n| n > 0).map(Some),
        _ => return None,
    })
}

fn eval_diff_reason(reason: CompletenessReason, diff: &DiffState) -> Option<Option<Option<u64>>> {
    Some(match reason {
        CompletenessReason::StaleIndex => diff.stale_index.then_some(None),
        CompletenessReason::UnindexedChanges => {
            (diff.unindexed_changes > 0).then_some(Some(diff.unindexed_changes))
        }
        CompletenessReason::ManifestChanged => diff.manifest_changed.then_some(None),
        _ => return None,
    })
}

fn skipped_count(skipped: Option<u64>) -> Option<Option<u64>> {
    match skipped {
        None => Some(None),
        Some(0) => None,
        Some(count) => Some(Some(count)),
    }
}

fn has_trait_dispatch(targets: &[SymbolFacts]) -> bool {
    targets
        .iter()
        .any(|target| target.kind == "method" && target.trait_name.is_some())
}

fn has_go_method(source: &impl ImpactSource, targets: &[SymbolFacts]) -> Result<bool, StoreError> {
    for target in targets {
        if target.kind == "method" && is_go_file(source, &target.file)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_external_users(
    source: &impl ImpactSource,
    targets: &[SymbolFacts],
) -> Result<bool, StoreError> {
    for target in targets {
        if target.visibility == "public"
            && target.target_kind.as_deref() == Some("lib")
            && !is_go_internal_or_main(source, target)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_go_file(source: &impl ImpactSource, path: &str) -> Result<bool, StoreError> {
    if path.ends_with(".go") {
        return Ok(true);
    }
    Ok(source.file_lang(path)?.as_deref() == Some("go"))
}

fn is_go_internal_or_main(
    source: &impl ImpactSource,
    target: &SymbolFacts,
) -> Result<bool, StoreError> {
    if !is_go_file(source, &target.file)? {
        return Ok(false);
    }
    Ok(target.package.as_deref() == Some("main") || is_internal_path(&target.file))
}

fn is_internal_path(path: &str) -> bool {
    path.starts_with("internal/") || path.contains("/internal/")
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
