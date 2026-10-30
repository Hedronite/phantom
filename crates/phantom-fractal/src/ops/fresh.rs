//! `meta`: how an index stands against its tree (SPEC-w9 G0c, D3).
//!
//! Every success envelope from an op that reads an index carries `meta`
//! after `data`, from the freshness probe: whether an index pass would
//! change the index, HEAD now and at the last pass, and the rank epoch.
//! The probe only reads, so a stale index still answers and is never
//! re-indexed by a read.

use std::path::Path;

use phantom_fractal_extract::{Freshness, probe};
use phantom_fractal_proto::Meta;
use phantom_fractal_store::Store;

use super::Failure;

/// The `meta` of `store`, the index of the canonical tree `root`. A tree
/// the probe cannot read is reported stale, with HEAD as far as it reads:
/// the answer still comes back, and nothing claims it is current.
pub(crate) fn meta(root: &Path, store: &Store) -> Result<Meta, Failure> {
    let rank_epoch = store.rank_epoch()?;
    Ok(match probe(root, store) {
        Ok(freshness) => of(&freshness, rank_epoch),
        Err(_) => Meta {
            stale: true,
            head: phantom_fractal_git::head(root).ok().flatten(),
            index_head: store.index_head()?,
            rank_epoch,
        },
    })
}

/// The `meta` a probe's answer gives.
pub(crate) fn of(freshness: &Freshness, rank_epoch: u64) -> Meta {
    Meta {
        stale: freshness.stale(),
        head: freshness.head.clone(),
        index_head: freshness.index_head.clone(),
        rank_epoch,
    }
}
