//! `meta`: how an index stands against its tree (SPEC-w9 G0c, D3).
//!
//! Every success envelope from an op that reads an index carries `meta`
//! after `data`, from the freshness probe: whether an index pass would
//! change the index, HEAD now and at the last pass, and the rank epoch.
//! The probe only reads, so a stale index still answers and is never
//! re-indexed by a read.

use std::path::Path;

use phantom_fractal_extract::{Freshness, IndexError, probe};
use phantom_fractal_proto::Meta;
use phantom_fractal_store::{IndexState, Store};

use super::Failure;

/// The `meta` of `store`, the index of the canonical tree `root`. A tree
/// the probe cannot read is reported stale, with HEAD as far as it reads:
/// the answer still comes back, and nothing claims it is current.
pub(crate) fn meta(root: &Path, store: &Store) -> Result<Meta, Failure> {
    let rank_epoch = store.rank_epoch()?;
    Ok(match probe(root, store) {
        Ok(freshness) => of(&freshness, rank_epoch),
        Err(_) => with_locality_guard(Meta {
            stale: true,
            head: phantom_fractal_git::head(root).ok().flatten(),
            index_head: store.index_head()?,
            rank_epoch,
            limit: None,
            stale_cause: None,
        }),
    })
}

/// The `meta` a probe's answer gives.
pub(crate) fn of(freshness: &Freshness, rank_epoch: u64) -> Meta {
    with_locality_guard(Meta {
        stale: freshness.stale(),
        head: freshness.head.clone(),
        index_head: freshness.index_head.clone(),
        rank_epoch,
        limit: None,
        stale_cause: None,
    })
}

/// Guarded freshness state for `path:line` resolution: stale whenever either the
/// index probe or the locality guard flags staleness.
pub(crate) fn index_state(root: &Path, store: &Store) -> Result<IndexState, IndexError> {
    let freshness = probe(root, store)?;
    let stale = freshness.stale() || locality_cause(freshness.head.as_deref()).is_some();
    Ok(if stale {
        IndexState::Stale
    } else {
        IndexState::Fresh
    })
}

fn is_valid_expected_head(head: &str) -> bool {
    (head.len() == 40 || head.len() == 64)
        && head
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn locality_cause(head: Option<&str>) -> Option<&'static str> {
    if let Some(expected) = std::env::var_os("PHANTOM_EXPECTED_HEAD") {
        let valid = expected.to_str().filter(|s| is_valid_expected_head(s));
        if valid.is_none() || head != valid {
            return Some("local_head");
        }
    }
    if let Some(dirty) = std::env::var_os("PHANTOM_EXPECTED_DIRTY")
        && dirty.to_str() != Some("0")
    {
        return Some("local_dirty");
    }
    None
}

fn with_locality_guard(mut meta: Meta) -> Meta {
    if let Some(cause) = locality_cause(meta.head.as_deref()) {
        meta.stale = true;
        meta.stale_cause = Some(cause.to_owned());
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_expected_head_lengths_and_lowercase_hex() {
        assert!(is_valid_expected_head(&"a".repeat(40)));
        assert!(is_valid_expected_head(&"0123456789abcdef".repeat(4)));
        assert!(!is_valid_expected_head(&"a".repeat(39)));
        assert!(!is_valid_expected_head(&"a".repeat(41)));
        assert!(!is_valid_expected_head(&"a".repeat(63)));
        assert!(!is_valid_expected_head(&"A".repeat(40)));
        assert!(!is_valid_expected_head(&format!("{}g", "a".repeat(39))));
    }
}
