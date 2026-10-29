//! The map's lines (SPEC-w7 G2, SPEC-w8 G2, canon §8.4).
//!
//! Every line is ASCII: a byte outside printable ASCII becomes `?`.

/// The longest symbol signature kept, `...` included.
pub(crate) const SIGNATURE_CHARS: usize = 100;

/// The hex digits of HEAD the header shows.
pub(crate) const HEAD_HEX: usize = 7;

/// `phantom-fractal map | budget B | used U | rank E`, with
/// `| head <7 hex>` after `phantom-fractal map` when a co-change scan
/// exists.
pub(crate) fn header(budget: u64, used: u64, epoch: u64, head: Option<&str>) -> String {
    match head {
        Some(head) => format!(
            "phantom-fractal map | head {} | budget {budget} | used {used} | rank {epoch}",
            ascii(head.get(..HEAD_HEX).unwrap_or(head))
        ),
        None => format!("phantom-fractal map | budget {budget} | used {used} | rank {epoch}"),
    }
}

/// `-- omitted: F files, S symbols (raise --budget or use --focus)`.
pub(crate) fn footer(files: u64, symbols: u64) -> String {
    format!("-- omitted: {files} files, {symbols} symbols (raise --budget or use --focus)")
}

/// `<path>  [r <rank>]`, the rank to three significant digits, then
/// `  also-changes: ` and the partner names joined by `, ` when there are
/// any.
pub(crate) fn file_line(path: &str, rank: f64, also_changes: &[String]) -> String {
    let line = format!("{}  [r {}]", ascii(path), significant(rank));
    if also_changes.is_empty() {
        return line;
    }
    let partners: Vec<String> = also_changes.iter().map(|name| ascii(name)).collect();
    format!("{line}  also-changes: {}", partners.join(", "))
}

/// Two spaces, the signature, two spaces, `:<line>`. A method's `fn name`
/// reads `fn Owner::name`; the signature is cut to [`SIGNATURE_CHARS`]
/// with `...`.
pub(crate) fn symbol_line(
    kind: &str,
    name: &str,
    owner: Option<&str>,
    signature: Option<&str>,
    line: Option<i64>,
) -> String {
    let signature = match signature.filter(|text| !text.trim().is_empty()) {
        Some(text) => text.to_owned(),
        None => format!("{kind} {name}"),
    };
    let signature = match (kind, owner) {
        ("method", Some(owner)) => qualify(&signature, name, owner),
        _ => signature,
    };
    let mut signature = ascii(&signature);
    if signature.len() > SIGNATURE_CHARS {
        signature.truncate(SIGNATURE_CHARS - 3);
        signature.push_str("...");
    }
    let line = line.map_or_else(|| "?".to_owned(), |line| line.to_string());
    format!("  {signature}  :{line}")
}

/// Four spaces, `/// `, then the doc summary.
pub(crate) fn doc_line(summary: &str) -> String {
    format!("    /// {}", ascii(summary))
}

/// `signature` with its first `fn name`, ending at a non-identifier
/// character, read as `fn owner::name`. A signature with no such `fn`, or
/// a method with no signature written as `method name`, is left as it is
/// or gets `owner::` before the name.
fn qualify(signature: &str, name: &str, owner: &str) -> String {
    let needle = format!("fn {name}");
    let mut from = 0;
    while let Some(found) = signature[from..].find(&needle) {
        let start = from + found;
        let end = start + needle.len();
        let boundary_before = signature[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_ident(ch));
        let boundary_after = signature[end..]
            .chars()
            .next()
            .is_none_or(|ch| !is_ident(ch));
        if boundary_before && boundary_after {
            return format!(
                "{}fn {owner}::{name}{}",
                &signature[..start],
                &signature[end..]
            );
        }
        from = end;
    }
    match signature.strip_suffix(name) {
        Some(head) if head.ends_with(' ') => format!("{head}{owner}::{name}"),
        _ => signature.to_owned(),
    }
}

fn is_ident(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// `text` with every byte outside printable ASCII, `0x20..=0x7e`, as `?`.
/// A multi-byte character becomes one `?` per byte.
pub(crate) fn ascii(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if (0x20..=0x7e).contains(&byte) {
                char::from(byte)
            } else {
                '?'
            }
        })
        .collect()
}

/// `value` to three significant digits in plain decimal: `0.0412`,
/// `0.500`, `1.00`. Zero, a negative value or NaN reads `0.00`.
pub(crate) fn significant(value: f64) -> String {
    if value.is_nan() || value <= 0.0 || value.is_infinite() {
        return "0.00".to_owned();
    }
    let scientific = format!("{value:.2e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return "0.00".to_owned();
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let Ok(exponent) = exponent.parse::<i32>() else {
        return "0.00".to_owned();
    };
    if exponent < 0 {
        let zeros = "0".repeat(exponent.unsigned_abs() as usize - 1);
        format!("0.{zeros}{digits}")
    } else {
        let point = (exponent as usize + 1).min(digits.len());
        let (whole, fraction) = digits.split_at(point);
        let pad = "0".repeat(exponent as usize + 1 - point);
        if fraction.is_empty() {
            format!("{whole}{pad}")
        } else {
            format!("{whole}{pad}.{fraction}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_significant_digits() {
        let cases = [
            (0.0412345, "0.0412"),
            (0.5, "0.500"),
            (1.0, "1.00"),
            (0.099_96, "0.100"),
            (0.000_012_34, "0.0000123"),
            (0.999_9, "1.00"),
            (0.0, "0.00"),
            (f64::NAN, "0.00"),
        ];
        for (value, want) in cases {
            assert_eq!(significant(value), want, "{value}");
        }
    }

    #[test]
    fn a_method_reads_owner_name_and_a_long_signature_is_cut() {
        assert_eq!(
            symbol_line(
                "method",
                "new",
                Some("Circle"),
                Some("pub fn new(r: f64) -> Self"),
                Some(21)
            ),
            "  pub fn Circle::new(r: f64) -> Self  :21"
        );
        // `fn newer` is not `fn new`.
        assert_eq!(
            qualify("fn newer() -> fn new()", "new", "T"),
            "fn newer() -> fn T::new()"
        );
        assert_eq!(
            symbol_line("method", "go", Some("T"), None, None),
            "  method T::go  :?"
        );
        let long = format!("pub fn f({})", "a: u8, ".repeat(30));
        let line = symbol_line("function", "f", None, Some(&long), Some(1));
        let signature = line.trim_start().split("  :").next().expect("signature");
        assert_eq!(signature.len(), 100);
        assert!(signature.ends_with("..."));
    }

    #[test]
    fn non_ascii_bytes_become_question_marks() {
        // Two bytes, a tab, `x`, then three bytes.
        assert_eq!(ascii("é\tx…"), "???x???");
        // `ö` and `ß` are two bytes each.
        assert_eq!(doc_line("Größe"), "    /// Gr????e");
        assert_eq!(
            file_line("src/naïve.rs", 0.25, &[]),
            "src/na??ve.rs  [r 0.250]"
        );
        let also = ["b.rs".to_owned(), "größe.rs".to_owned()];
        assert_eq!(
            file_line("a.rs", 0.5, &also),
            "a.rs  [r 0.500]  also-changes: b.rs, gr????e.rs"
        );
    }
}
