//! `max_tokens`: one read reply fitted to a token budget (SPEC-w8 G0c).
//!
//! The reply is the compact envelope line the command line prints. It
//! fits when `estimate_tokens(line) <= packing_budget(max_tokens)`. A
//! reply that does not fit drops entries one at a time, in its op's fixed
//! order, until it does; [`Shed`] gives that order. With a budget, `data`
//! ends with `tokens`, whose `used` is the estimate of the final line,
//! `tokens` and `meta` included. Without one nothing is added.

use phantom_fractal_analyze::{estimate_tokens, packing_budget};
use phantom_fractal_proto::{Envelope, ErrorKind, Meta};
use serde::Serialize;

use super::Failure;

/// The smallest and largest `max_tokens`.
pub(crate) const MAX_TOKENS_MIN: u64 = 1;
pub(crate) const MAX_TOKENS_MAX: u64 = 100_000;

/// `data.tokens`: the budget asked for, the estimate of the printed line,
/// and how many entries were dropped to reach it.
#[derive(Clone, Copy, Serialize)]
pub(crate) struct Tokens {
    max: u64,
    used: u64,
    omitted: u64,
}

/// A reply that can drop entries to fit a budget.
pub(crate) trait Shed: Serialize + Clone {
    /// How many entries can be dropped.
    fn sheddable(&self) -> usize;
    /// Drops the first `count` entries in the op's order. The entry order
    /// is fixed, so dropping more never brings one back.
    fn shed(&mut self, count: usize);
    fn set_tokens(&mut self, tokens: Tokens);
}

/// `Usage` unless `max_tokens` is absent or in range.
pub(crate) fn check(max_tokens: Option<u64>) -> Result<(), Failure> {
    match max_tokens {
        Some(max) if !(MAX_TOKENS_MIN..=MAX_TOKENS_MAX).contains(&max) => Err(Failure::new(
            ErrorKind::Usage,
            format!("max_tokens must be {MAX_TOKENS_MIN} to {MAX_TOKENS_MAX}"),
        )),
        _ => Ok(()),
    }
}

/// `data` as is without a budget. With one, `data` after the fewest drops
/// that fit in a line that carries `meta`, with `tokens` set; `Usage`
/// `budget_too_small` when it does not fit with every entry dropped.
pub(crate) fn fit<T: Shed>(data: T, max_tokens: Option<u64>, meta: &Meta) -> Result<T, Failure> {
    let Some(max) = max_tokens else {
        return Ok(data);
    };
    let room = packing_budget(max);
    let trimmed = |count: usize| -> Result<(T, u64), Failure> {
        let mut reply = data.clone();
        reply.shed(count);
        let used = settle(&mut reply, max, count, meta)?;
        Ok((reply, used))
    };
    let total = data.sheddable();
    let (_, needed) = trimmed(total)?;
    if needed > room {
        return Err(Failure::new(
            ErrorKind::Usage,
            format!(
                "budget_too_small: max_tokens {max} is too small: the reply needs {needed} \
                 estimated tokens with every list empty, and 95% of max_tokens is {room}"
            ),
        ));
    }
    // A drop takes out at least a whole entry and its comma, and the
    // counts can grow by at most a digit each, so the estimate never rises
    // with more drops: the fewest drops that fit can be found by halving.
    let (mut low, mut high) = (0, total);
    while low < high {
        let middle = low + (high - low) / 2;
        if trimmed(middle)?.1 <= room {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    trimmed(high).map(|(reply, _)| reply)
}

/// Sets `tokens` on `reply` so that `used` is the estimate of the line it
/// prints with `meta`, and returns that estimate. Each pass can only add
/// digits, so `used` rises until it holds still.
fn settle<T: Shed>(reply: &mut T, max: u64, omitted: usize, meta: &Meta) -> Result<u64, Failure> {
    let omitted = u64::try_from(omitted).unwrap_or(u64::MAX);
    let mut used = 0;
    loop {
        reply.set_tokens(Tokens { max, used, omitted });
        let line = serde_json::to_string(&Envelope::success_with_meta(&*reply, meta.clone()))
            .map_err(|_| Failure::new(ErrorKind::Usage, "the reply could not be encoded"))?;
        let estimate = estimate_tokens(&line);
        if estimate == used {
            return Ok(used);
        }
        used = estimate;
    }
}

/// Drops up to `count` items from the end of `items`, and takes them off
/// `count`.
pub(crate) fn drop_last<T>(items: &mut Vec<T>, count: &mut usize) {
    let take = (*count).min(items.len());
    items.truncate(items.len() - take);
    *count -= take;
}
