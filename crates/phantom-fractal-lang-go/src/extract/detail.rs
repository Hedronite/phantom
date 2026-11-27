//! Line numbers, signatures, doc summaries, and type-name cleaners for Go AST nodes.

use phantom_fractal_lang::{DOC_SUMMARY_CHARS, SIGNATURE_CHARS, Visibility};
use tree_sitter::Node;

/// Returns 1-based start line of `node`.
pub(crate) fn start_line(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().row.saturating_add(1)).unwrap_or(u32::MAX)
}

/// Returns 1-based start column (byte offset within line) of `node`.
pub(crate) fn start_col(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().column.saturating_add(1)).unwrap_or(u32::MAX)
}

/// Returns 1-based line of `node`'s last byte.
pub(crate) fn end_line(node: Node<'_>) -> u32 {
    u32::try_from(node.end_position().row.saturating_add(1)).unwrap_or(u32::MAX)
}

/// Extracts UTF-8 source text of `node`.
pub(crate) fn node_text(node: Node<'_>, src: &[u8]) -> String {
    let bytes = src.get(node.byte_range()).unwrap_or_default();
    String::from_utf8_lossy(bytes).into_owned()
}

/// Extracts signature from `node` start up to its `body` (if any), whitespace-collapsed.
pub(crate) fn signature(node: Node<'_>, src: &[u8]) -> String {
    let end = node
        .child_by_field_name("body")
        .map_or_else(|| node.end_byte(), |b| b.start_byte());
    let text = String::from_utf8_lossy(src.get(node.start_byte()..end).unwrap_or_default());
    cap(&clean(&text), SIGNATURE_CHARS)
}

/// Extracts the first sentence of the contiguous Go doc comment directly above `node`.
pub(crate) fn doc_summary(node: Node<'_>, src: &[u8]) -> Option<String> {
    let lines = doc_lines(node, src);
    let para: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .skip_while(|l| l.trim().is_empty())
        .take_while(|l| !l.trim().is_empty())
        .collect();
    let joined = clean(&para.join(" "));
    let sentence = match joined.find(". ") {
        Some(idx) => &joined[..=idx],
        None => &joined,
    };
    (!sentence.is_empty()).then(|| cap(sentence, DOC_SUMMARY_CHARS))
}

fn doc_lines(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur = node.prev_named_sibling();
    let mut next_row = node.start_position().row;
    while let Some(sib) = cur.and_then(|n| contiguous_doc_comment(n, next_row, src)) {
        blocks.push(sib.lines);
        next_row = sib.start_row;
        cur = sib.prev;
    }
    blocks.into_iter().rev().flatten().collect()
}

struct DocBlock<'t> {
    lines: Vec<String>,
    start_row: usize,
    prev: Option<Node<'t>>,
}

fn contiguous_doc_comment<'t>(node: Node<'t>, next_row: usize, src: &[u8]) -> Option<DocBlock<'t>> {
    if node.kind() != "comment" || node.end_position().row + 1 < next_row {
        return None;
    }
    let text = node_text(node, src);
    if text.starts_with("//go:") || text.starts_with("// +build") {
        return None;
    }
    Some(DocBlock {
        lines: comment_lines(&text),
        start_row: node.start_position().row,
        prev: node.prev_named_sibling(),
    })
}

fn comment_lines(text: &str) -> Vec<String> {
    if let Some(rest) = text.strip_prefix("//") {
        return vec![rest.trim_start().to_owned()];
    }
    let inner = text
        .strip_prefix("/*")
        .and_then(|s| s.strip_suffix("*/"))
        .unwrap_or(text);
    inner
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim().to_owned())
        .collect()
}

/// Go exportedness: uppercase first char is `Public`, else `Private`.
pub(crate) fn go_visibility(name: &str) -> Visibility {
    if name.chars().next().is_some_and(char::is_uppercase) {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

/// True when `name` matches Go test/benchmark/fuzz/example conventions.
pub(crate) fn is_go_test_fn(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"]
        .iter()
        .any(|prefix| name.starts_with(prefix) && name.len() > prefix.len())
}

/// Strips whitespace, leading `*`, parens, and trailing `[...]` type arguments.
pub(crate) fn bare_type_name(raw: &str) -> String {
    let compact = strip_whitespace(raw);
    let no_ptr = compact.trim_start_matches(['*', '(', ')']);
    let no_generics = no_ptr.split('[').next().unwrap_or(no_ptr);
    no_generics.trim_end_matches(')').to_owned()
}

/// Strips whitespace from `raw`.
pub(crate) fn strip_whitespace(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Returns trailing selector segment of a type name (e.g. `"pkg.Type"` -> `"Type"`).
pub(crate) fn type_last_segment(type_name: &str) -> String {
    type_name.rsplit('.').next().unwrap_or(type_name).to_owned()
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    #[test]
    fn extracts_docs_stopping_at_go_directives_and_truncates() {
        let src = "package p\n\n//go:generate tool\n// First. Second.\nfunc F() {}\n\n// Gap doc.\n\nfunc G() {\n  _ = 1\n}\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        let mut cur = root.walk();
        let fns: Vec<Node<'_>> = root
            .children(&mut cur)
            .filter(|n| n.kind() == "function_declaration")
            .collect();
        assert_eq!(
            doc_summary(fns[0], src.as_bytes()).as_deref(),
            Some("First.")
        );
        assert_eq!(doc_summary(fns[1], src.as_bytes()), None);
        assert_eq!(start_line(fns[1]), 9);
        assert_eq!(end_line(fns[1]), 11);
        assert_eq!(start_col(fns[1]), 1);
        let stmt = fns[1]
            .child_by_field_name("body")
            .unwrap()
            .named_child(0)
            .unwrap();
        assert!(start_col(stmt) > 1);
        assert!(is_go_test_fn("TestA"));
        assert!(!is_go_test_fn("Test"));
        assert!(!is_go_test_fn("Helper"));
        let long = "a".repeat(200);
        assert!(cap(&long, 10).ends_with('…'));
    }
}
