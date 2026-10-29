//! End line, signature, and doc summary of an item or method node
//! (SPEC-w6 G0a). Heuristics over the tree and the source text only.

use phantom_fractal_lang::{DOC_SUMMARY_CHARS, SIGNATURE_CHARS};
use tree_sitter::Node;

/// The three detail fields of one item or method.
pub(crate) struct Detail {
    pub(crate) end_line: u32,
    pub(crate) signature: String,
    pub(crate) doc_summary: Option<String>,
}

pub(crate) fn detail(item: Node<'_>, src: &[u8]) -> Detail {
    Detail {
        end_line: end_line(item),
        signature: signature(item, src),
        doc_summary: doc_summary(item, src),
    }
}

/// The 1-based line of the node's last byte.
fn end_line(node: Node<'_>) -> u32 {
    let end = node.end_position();
    // An end at column 0 sits just past a newline: the last byte is on the
    // line before.
    let row = if end.column == 0 && node.end_byte() > node.start_byte() {
        end.row
    } else {
        end.row.saturating_add(1)
    };
    u32::try_from(row).unwrap_or(u32::MAX)
}

/// The text from the node's start to its body, or to its closing `;`.
fn signature(item: Node<'_>, src: &[u8]) -> String {
    let end = match item.child_by_field_name("body") {
        Some(body) => body.start_byte(),
        None => {
            let last = item.child(item.child_count().saturating_sub(1));
            match last.filter(|last| last.kind() == ";") {
                Some(semicolon) => semicolon.start_byte(),
                None => item.end_byte(),
            }
        }
    };
    let text = src.get(item.start_byte()..end).unwrap_or_default();
    cap(&clean(&String::from_utf8_lossy(text)), SIGNATURE_CHARS)
}

/// The first sentence of the first paragraph of the outer docs above
/// `item`.
fn doc_summary(item: Node<'_>, src: &[u8]) -> Option<String> {
    let lines = doc_lines(item, src);
    let paragraph: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty())
        .collect();
    let text = clean(&paragraph.join("\n"));
    let sentence = match text.find(". ") {
        Some(dot) => &text[..=dot],
        None => &text,
    };
    (!sentence.is_empty()).then(|| cap(sentence, DOC_SUMMARY_CHARS))
}

/// The text lines of the outer doc comments above `item`, in source
/// order. Attributes and non-doc comments between them are passed over;
/// any other node ends the run.
fn doc_lines(item: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut docs = Vec::new();
    let mut sibling = item.prev_named_sibling();
    while let Some(node) = sibling {
        match node.kind() {
            "attribute_item" => {}
            "line_comment" | "block_comment" => {
                let text = src.get(node.byte_range()).unwrap_or_default();
                let text = String::from_utf8_lossy(text);
                if let Some(lines) = outer_doc(&text) {
                    docs.push(lines);
                }
            }
            _ => break,
        }
        sibling = node.prev_named_sibling();
    }
    docs.into_iter().rev().flatten().collect()
}

/// The lines of one comment when it is an outer doc comment: `///` but not
/// `////`, or `/**` but not `/***` or `/**/`.
fn outer_doc(comment: &str) -> Option<Vec<String>> {
    if let Some(rest) = comment.strip_prefix("///") {
        if rest.starts_with('/') {
            return None;
        }
        return Some(vec![rest.trim_end_matches(['\r', '\n']).to_owned()]);
    }
    let body = comment.strip_prefix("/**")?;
    if body.starts_with('*') || body.starts_with('/') {
        return None;
    }
    let body = body.strip_suffix("*/").unwrap_or(body);
    Some(block_lines(body))
}

/// A block doc's lines, with the leading `*` of each line after the first
/// removed when every non-blank one has it.
fn block_lines(body: &str) -> Vec<String> {
    let lines: Vec<&str> = body.lines().collect();
    let starred = lines
        .iter()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.trim_start().starts_with('*'));
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trimmed = line.trim_start();
            match trimmed.strip_prefix('*') {
                Some(rest) if starred && index > 0 => rest.to_owned(),
                _ => (*line).to_owned(),
            }
        })
        .collect()
}

/// Control characters other than whitespace removed, then each whitespace
/// run collapsed to one space, trimmed.
fn clean(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `chars` chars, with `…` appended when cut.
fn cap(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_counts_chars_not_bytes() {
        assert_eq!(cap("abc", 3), "abc");
        assert_eq!(cap("abcd", 3), "abc…");
        assert_eq!(cap("ééé", 2), "éé…");
        assert_eq!(cap("", 0), "");
        assert_eq!(cap("a", 0), "…");
    }

    #[test]
    fn clean_drops_control_and_collapses_whitespace() {
        assert_eq!(clean("  a\t\n b\u{7}c \u{0}"), "a bc");
        assert_eq!(clean("a\u{7} \u{1b}b"), "a b");
    }

    #[test]
    fn only_outer_doc_comments_count() {
        assert_eq!(outer_doc("/// a\n"), Some(vec![" a".to_owned()]));
        assert_eq!(outer_doc("///"), Some(vec![String::new()]));
        assert_eq!(outer_doc("//// rule"), None);
        assert_eq!(outer_doc("//! inner"), None);
        assert_eq!(outer_doc("// plain"), None);
        assert_eq!(outer_doc("/** a */"), Some(vec![" a ".to_owned()]));
        assert_eq!(outer_doc("/*** rule */"), None);
        assert_eq!(outer_doc("/**/"), None);
        assert_eq!(outer_doc("/*! inner */"), None);
        assert_eq!(outer_doc("/* plain */"), None);
    }

    #[test]
    fn block_stars_come_off_only_when_every_line_has_one() {
        assert_eq!(block_lines("\n * a\n * b\n "), ["", " a", " b", " "]);
        assert_eq!(block_lines("\n * a\n b\n"), ["", " * a", " b"]);
    }
}
