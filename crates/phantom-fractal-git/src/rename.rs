//! Phantom's own rename pairing (SPEC-w8 G1c, the D2 amendment).
//!
//! Within one commit's diff, each deleted blob may pair with one added
//! blob. The same object id scores 100%. Otherwise the score is the
//! span-hash similarity: both blobs are cut into chunks that end at `\n`
//! or after 64 bytes, each distinct chunk's bytes are counted, and the
//! bytes the two share (the smaller count of each chunk) are divided by
//! the larger blob's size. Pairs below 50% are dropped, and the rest are
//! taken greedily, best score first, ties by the deleted path and then
//! the added path. This is the estimator family of `git -M50%`; it does
//! not claim parity with git.

use std::collections::BTreeMap;

/// The lowest similarity, in percent, that pairs a rename.
pub const MIN_SIMILARITY_PERCENT: u64 = 50;

/// The longest chunk, in bytes.
const CHUNK: usize = 64;

/// Inexact scoring is tried only when the deleted and added counts
/// multiply to at most this; past it, only identical blobs pair.
const MAX_CANDIDATES: usize = 10_000;

/// Blobs larger than this, in bytes, pair only when identical.
const MAX_BLOB: usize = 1 << 20;

/// One side of a candidate rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Side<K> {
    pub path: String,
    pub oid: K,
}

/// The span-hash similarity of `old` and `new`, from 0 to 1. Two empty
/// blobs score 1.
pub fn similarity(old: &[u8], new: &[u8]) -> f64 {
    let larger = old.len().max(new.len());
    if larger == 0 {
        return 1.0;
    }
    shared(&spans(old), &spans(new)) as f64 / larger as f64
}

/// Each distinct chunk's hash with its total bytes.
fn spans(bytes: &[u8]) -> BTreeMap<u64, u64> {
    let mut spans = BTreeMap::new();
    let mut start = 0;
    while start < bytes.len() {
        let rest = &bytes[start..];
        let len = match rest.iter().take(CHUNK).position(|byte| *byte == b'\n') {
            Some(newline) => newline + 1,
            None => rest.len().min(CHUNK),
        };
        *spans.entry(fnv1a(&rest[..len])).or_insert(0) += len as u64;
        start += len;
    }
    spans
}

/// The bytes two span sets share.
fn shared(old: &BTreeMap<u64, u64>, new: &BTreeMap<u64, u64>) -> u64 {
    old.iter()
        .filter_map(|(hash, count)| new.get(hash).map(|other| (*count).min(*other)))
        .sum()
}

/// 64-bit FNV-1a.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The renames among `deleted` and `added`, as index pairs into them.
/// `read` gives a blob's bytes, or `None` when it cannot be read; such a
/// blob pairs only when identical.
pub(crate) fn pair<K: Eq>(
    deleted: &[Side<K>],
    added: &[Side<K>],
    mut read: impl FnMut(&K) -> Option<Vec<u8>>,
) -> Vec<(usize, usize)> {
    if deleted.is_empty() || added.is_empty() {
        return Vec::new();
    }
    // (score as shared bytes over the larger size, deleted, added)
    let mut candidates: Vec<(u64, u64, usize, usize)> = Vec::new();
    let inexact = deleted.len().saturating_mul(added.len()) <= MAX_CANDIDATES;
    let mut blobs = |sides: &[Side<K>]| -> Vec<Option<(usize, BTreeMap<u64, u64>)>> {
        sides
            .iter()
            .map(|side| {
                read(&side.oid)
                    .filter(|bytes| bytes.len() <= MAX_BLOB)
                    .map(|bytes| (bytes.len(), spans(&bytes)))
            })
            .collect()
    };
    let (old, new) = if inexact {
        (blobs(deleted), blobs(added))
    } else {
        (Vec::new(), Vec::new())
    };
    for (d, from) in deleted.iter().enumerate() {
        for (a, to) in added.iter().enumerate() {
            if from.oid == to.oid {
                candidates.push((1, 1, d, a));
                continue;
            }
            let (Some(Some((old_len, old_spans))), Some(Some((new_len, new_spans)))) =
                (old.get(d), new.get(a))
            else {
                continue;
            };
            let larger = (*old_len).max(*new_len) as u64;
            let smaller = (*old_len).min(*new_len) as u64;
            // The shared bytes cannot exceed the smaller blob.
            if larger == 0 || smaller * 100 < larger * MIN_SIMILARITY_PERCENT {
                continue;
            }
            let common = shared(old_spans, new_spans);
            if common * 100 >= larger * MIN_SIMILARITY_PERCENT {
                candidates.push((common, larger, d, a));
            }
        }
    }
    candidates.sort_by(|x, y| {
        // Higher score first, compared exactly: x.0/x.1 against y.0/y.1.
        (u128::from(y.0) * u128::from(x.1))
            .cmp(&(u128::from(x.0) * u128::from(y.1)))
            .then_with(|| deleted[x.2].path.cmp(&deleted[y.2].path))
            .then_with(|| added[x.3].path.cmp(&added[y.3].path))
    });
    let mut used_old = vec![false; deleted.len()];
    let mut used_new = vec![false; added.len()];
    let mut pairs = Vec::new();
    for (_, _, d, a) in candidates {
        if used_old[d] || used_new[a] {
            continue;
        }
        used_old[d] = true;
        used_new[a] = true;
        pairs.push((d, a));
    }
    pairs.sort_unstable();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(path: &str, oid: u32) -> Side<u32> {
        Side {
            path: path.to_owned(),
            oid,
        }
    }

    fn lines(range: std::ops::Range<u32>) -> Vec<u8> {
        // Every line is 10 bytes, so a share of lines is the same share of bytes.
        range
            .map(|n| format!("line {n:04}\n"))
            .collect::<String>()
            .into_bytes()
    }

    #[test]
    fn similarity_counts_shared_chunks_over_the_larger_blob() {
        assert_eq!(similarity(b"", b""), 1.0);
        assert_eq!(similarity(b"a\n", b""), 0.0);
        assert_eq!(similarity(b"a\nb\n", b"a\nb\n"), 1.0);
        // Two of four equal-length lines kept.
        assert_eq!(similarity(b"a\nb\nc\nd\n", b"a\nb\nx\ny\n"), 0.5);
        // Half the bytes, all shared: 4 of 8.
        assert_eq!(similarity(b"a\nb\nc\nd\n", b"a\nb\n"), 0.5);
        // A repeated chunk counts once per copy, up to the smaller count.
        assert_eq!(similarity(b"a\na\na\na\n", b"a\na\n"), 0.5);
        // A line longer than 64 bytes is cut into 64-byte chunks.
        let long = vec![b'x'; 128];
        let mut changed = long.clone();
        changed[100] = b'y';
        assert_eq!(similarity(&long, &changed), 0.5);
    }

    #[test]
    fn identical_oids_pair_first_and_ties_go_by_path() {
        let deleted = [side("b.rs", 1), side("a.rs", 1)];
        let added = [side("z.rs", 1), side("y.rs", 1)];
        let pairs = pair(&deleted, &added, |_| Some(b"same\n".to_vec()));
        // a.rs (index 1) takes y.rs (index 1); b.rs takes z.rs.
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn half_similar_pairs_and_just_under_half_does_not() {
        let blobs = |oid: &u32| {
            Some(match oid {
                1 => lines(0..100),
                2 => [lines(0..50), lines(200..250)].concat(),
                3 => [lines(0..49), lines(300..351)].concat(),
                _ => unreachable!(),
            })
        };
        let deleted = [side("old.rs", 1)];
        assert_eq!(pair(&deleted, &[side("new.rs", 2)], blobs), vec![(0, 0)]);
        assert_eq!(pair(&deleted, &[side("new.rs", 3)], blobs), vec![]);
    }

    #[test]
    fn the_best_score_wins_a_contested_blob() {
        let blobs = |oid: &u32| {
            Some(match oid {
                1 => lines(0..100),
                2 => [lines(0..90), lines(500..510)].concat(),
                3 => [lines(0..60), lines(600..640)].concat(),
                _ => unreachable!(),
            })
        };
        let deleted = [side("old.rs", 1)];
        let added = [side("a.rs", 3), side("b.rs", 2)];
        assert_eq!(pair(&deleted, &added, blobs), vec![(0, 1)]);
    }

    #[test]
    fn unreadable_or_oversized_blobs_pair_only_when_identical() {
        let deleted = [side("old.rs", 1), side("big.rs", 4)];
        let added = [side("new.rs", 2), side("big2.rs", 5)];
        let pairs = pair(&deleted, &added, |oid| match oid {
            1 | 2 => None,
            _ => Some(vec![b'x'; MAX_BLOB + 1]),
        });
        assert_eq!(pairs, vec![]);
    }

    #[test]
    fn past_the_candidate_cap_only_identical_blobs_pair() {
        let deleted: Vec<Side<u32>> = (0..101).map(|n| side(&format!("d{n:03}"), n)).collect();
        let mut added: Vec<Side<u32>> = (0..100)
            .map(|n| side(&format!("a{n:03}"), 1_000 + n))
            .collect();
        added.push(side("same", 7));
        let pairs = pair(&deleted, &added, |_| Some(b"identical text\n".to_vec()));
        assert_eq!(pairs, vec![(7, 100)]);
        // Under the cap the same bytes pair by content.
        let pairs = pair(&deleted[..2], &added[..2], |_| {
            Some(b"identical text\n".to_vec())
        });
        assert_eq!(pairs, vec![(0, 0), (1, 1)]);
    }
}
