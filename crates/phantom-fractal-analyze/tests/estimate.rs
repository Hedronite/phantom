//! `estimate_tokens`, the estimator the map and budgeted replies share
//! (SPEC-w8 G0a).

use phantom_fractal_analyze::estimate_tokens;

/// `ceil(bytes / 3.5) + 1` for one line, written out again here.
fn line(bytes: usize) -> u64 {
    (bytes as f64 / 3.5).ceil() as u64 + 1
}

#[test]
fn one_line_is_ceil_bytes_over_three_and_a_half_plus_one() {
    for (bytes, want) in [(1, 2), (3, 2), (4, 3), (7, 3), (8, 4), (35, 11), (36, 12)] {
        assert_eq!(estimate_tokens(&"x".repeat(bytes)), want, "{bytes}");
    }
    for bytes in 1..2_000 {
        assert_eq!(estimate_tokens(&"x".repeat(bytes)), line(bytes), "{bytes}");
    }
}

#[test]
fn lines_are_summed_and_a_final_newline_starts_none() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("\n"), 1);
    assert_eq!(estimate_tokens("\n\n"), 2);
    assert_eq!(estimate_tokens("abcd"), 3);
    assert_eq!(estimate_tokens("abcd\n"), 3);
    assert_eq!(estimate_tokens("abcd\n\n"), 4);
    assert_eq!(estimate_tokens("abcd\nabcdefgh"), 3 + 4);
    assert_eq!(estimate_tokens("abcd\nabcdefgh\n"), 3 + 4);
    assert_eq!(estimate_tokens("\nabcd"), 1 + 3);
    let text = ["", "a", "abcdefgh", "", "x".repeat(71).as_str()].join("\n");
    assert_eq!(estimate_tokens(&text), 1 + 2 + 4 + 1 + line(71));
}

#[test]
fn every_byte_but_the_newline_counts() {
    // A carriage return is a byte of its line.
    assert_eq!(estimate_tokens("abc\r\n"), line(4));
    assert_eq!(estimate_tokens("abc\n"), line(3));
    // Bytes, not chars: four three-byte chars are twelve bytes.
    assert_eq!(estimate_tokens("日本日本"), line(12));
    assert_eq!(estimate_tokens("🦀"), line(4));
}
