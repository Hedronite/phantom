//! Outer attributes, and the test marking they give (SPEC-w5 G1a).

use tree_sitter::Node;

use crate::compact;

/// The `attribute` nodes of the outer attributes on `item`, nearest first.
/// Comments between them are skipped.
pub(crate) fn outer(item: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut sibling = item.prev_named_sibling();
    while let Some(node) = sibling {
        match node.kind() {
            "attribute_item" => out.extend(node.named_child(0)),
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        sibling = node.prev_named_sibling();
    }
    out
}

/// True when `item` carries `#[cfg(test)]` exactly, whitespace removed.
pub(crate) fn cfg_test(item: Node<'_>, src: &[u8]) -> bool {
    outer(item)
        .into_iter()
        .any(|attribute| is_cfg_test(attribute, src))
}

/// [`Item::is_test`](phantom_fractal_lang::Item::is_test) for an item or
/// method node: a test attribute, `#[cfg(test)]` on the item, or an
/// enclosing inline `mod` with `#[cfg(test)]`.
pub(crate) fn is_test(item: Node<'_>, src: &[u8]) -> bool {
    outer(item)
        .into_iter()
        .any(|attribute| is_test_attribute(attribute, src) || is_cfg_test(attribute, src))
        || in_cfg_test_module(item, src)
}

fn is_cfg_test(attribute: Node<'_>, src: &[u8]) -> bool {
    compact(attribute, src) == "cfg(test)"
}

/// An attribute whose path's last segment is `test`, with or without
/// arguments: `#[test]`, `#[tokio::test]`, `#[tokio::test(flavor = "..")]`.
fn is_test_attribute(attribute: Node<'_>, src: &[u8]) -> bool {
    let Some(path) = attribute.named_child(0) else {
        return false;
    };
    let last = match path.kind() {
        "identifier" => Some(path),
        "scoped_identifier" => path.child_by_field_name("name"),
        _ => None,
    };
    last.is_some_and(|name| compact(name, src) == "test")
}

fn in_cfg_test_module(node: Node<'_>, src: &[u8]) -> bool {
    let mut parent = node.parent();
    while let Some(ancestor) = parent {
        if ancestor.kind() == "mod_item" && cfg_test(ancestor, src) {
            return true;
        }
        parent = ancestor.parent();
    }
    false
}
