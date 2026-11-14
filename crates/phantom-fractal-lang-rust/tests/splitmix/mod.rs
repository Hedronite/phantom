//! A std-only splitmix64 generator for the seeded fuzz and property runs
//! (SPEC-w13 G0b, D5).
//!
//! Case `n` draws from [`Rng::case`]`(n)` alone, so every failing case
//! replays from its number.
// Each test binary that includes this module uses a different part of it.
#![allow(dead_code)]

/// splitmix64.
pub struct Rng(u64);

impl Rng {
    /// The generator of case `number`.
    pub fn case(number: u64) -> Self {
        Self(number)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, for `n` above 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// Uniform in `low..=high`, for `high` below `u64::MAX`.
    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }

    /// Uniform in `0..len`, for `len` above 0.
    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// True `percent` times in 100.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// One of `items`, which is not empty.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.index(items.len())]
    }

    pub fn byte(&mut self) -> u8 {
        self.next().to_le_bytes()[0]
    }
}

/// The number in the environment variable `name`, or `default` when it is
/// unset. A value that is not a number fails the run.
pub fn env_number(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Err(_) => default,
        Ok(text) => text
            .parse()
            .unwrap_or_else(|_| panic!("{name}={text:?} is not a number")),
    }
}

#[test]
fn case_zero_draws_the_published_splitmix64_sequence() {
    let mut rng = Rng::case(0);
    assert_eq!(rng.next(), 0xe220_a839_7b1d_cdaf);
    assert_eq!(rng.next(), 0x6e78_9e6a_a1b9_65f4);
}
