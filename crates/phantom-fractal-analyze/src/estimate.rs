//! The token estimator (SPEC-w8 G0a, canon §8.3).

/// The estimated tokens of `text`: `ceil(bytes / 3.5) + 1` for each line,
/// summed.
///
/// Lines end at `\n`, which is not counted; every other byte is, `\r`
/// included. A final `\n` ends the last line and does not start another,
/// so `"a\n"` is one line and `"a\n\n"` is two. An empty text has no
/// lines and costs 0; an empty line costs 1.
pub fn estimate_tokens(text: &str) -> u64 {
    text.split_terminator('\n')
        .map(|line| (2 * line.len() as u64).div_ceil(7) + 1)
        .sum()
}
