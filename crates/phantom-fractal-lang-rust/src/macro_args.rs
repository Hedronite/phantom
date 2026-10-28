//! Call-shaped names inside macro token trees (SPEC-w5 G1a).
//!
//! A token tree is a flat run of tokens and nested token trees. Nothing is
//! parsed as an expression and nothing is expanded: a name counts when the
//! tokens right before a parenthesised token tree spell a path or a method
//! name. A `macro_rules!` body inside the tokens is not scanned.

use tree_sitter::Node;

/// One call-shaped name.
pub(crate) struct CallShape<'tree> {
    /// The path or method name before the parentheses, whitespace removed.
    pub text: String,
    /// The last identifier of the path, or the method name.
    pub last_segment: String,
    /// The first token of the path, or the method name.
    pub at: Node<'tree>,
}

/// Every call-shaped name in the token trees of `invocation`, nested
/// token trees included, in no particular order.
pub(crate) fn call_shapes<'tree>(invocation: Node<'tree>, src: &[u8]) -> Vec<CallShape<'tree>> {
    let mut out = Vec::new();
    let mut cursor = invocation.walk();
    let mut stack: Vec<Node<'tree>> = invocation
        .children(&mut cursor)
        .filter(|child| child.kind() == "token_tree")
        .collect();
    while let Some(tree) = stack.pop() {
        let mut cursor = tree.walk();
        let tokens: Vec<Node<'tree>> = tree.children(&mut cursor).collect();
        for (index, token) in tokens.iter().enumerate() {
            if token.kind() != "token_tree" || is_macro_rules_body(&tokens[..index], src) {
                continue;
            }
            stack.push(*token);
            if token.child(0).is_some_and(|open| open.kind() == "(")
                && let Some(shape) = shape(&tokens[..index], src)
            {
                out.push(shape);
            }
        }
    }
    out
}

/// True when the tokens end in `macro_rules ! name`.
fn is_macro_rules_body(before: &[Node<'_>], src: &[u8]) -> bool {
    let [.., keyword, bang, name] = before else {
        return false;
    };
    name.kind() == "identifier"
        && bang.kind() == "!"
        && keyword.kind() == "identifier"
        && keyword.utf8_text(src) == Ok("macro_rules")
}

/// The call shape the tokens `before` an opening parenthesis end in, if
/// any: `f`, `a::f`, `::a::f`, `crate::f`, `Self::new`, `f::<T>`,
/// `Vec::<u8>::new`, or `.m` and `.m::<T>`.
fn shape<'tree>(before: &[Node<'tree>], src: &[u8]) -> Option<CallShape<'tree>> {
    let end = *before.last()?;
    let mut at = before.len() - 1;
    if is_close_angle(end) {
        at = turbofish_segment(before, at)?;
    }
    let name = before[at];
    if name.kind() != "identifier" {
        return None;
    }
    let last_segment = text(name, name, src);
    if at > 0 && before[at - 1].kind() == "." {
        return Some(CallShape {
            text: text(name, end, src),
            last_segment,
            at: name,
        });
    }
    let mut start = at;
    while start > 0 && before[start - 1].kind() == "::" {
        let Some(prev) = start.checked_sub(2) else {
            start -= 1;
            break;
        };
        if is_segment(before[prev]) {
            start = prev;
        } else if is_close_angle(before[prev]) {
            start = turbofish_segment(before, prev)?;
        } else {
            start -= 1;
            break;
        }
    }
    Some(CallShape {
        text: text(before[start], end, src),
        last_segment,
        at: before[start],
    })
}

/// For generic arguments that close at `close`, the index of the path
/// segment before their `::<`, or none when they are not a turbofish.
fn turbofish_segment(before: &[Node<'_>], close: usize) -> Option<usize> {
    let open = open_angle(before, close)?;
    let colons = open.checked_sub(1)?;
    let segment = open.checked_sub(2)?;
    (before[colons].kind() == "::" && is_segment(before[segment])).then_some(segment)
}

/// The index of the `<` that matches the `>` or `>>` at `close`.
fn open_angle(before: &[Node<'_>], close: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for index in (0..=close).rev() {
        match before[index].kind() {
            ">" => depth += 1,
            ">>" => depth += 2,
            "<" => depth = depth.checked_sub(1)?,
            "<<" => depth = depth.checked_sub(2)?,
            _ => {}
        }
        if depth == 0 {
            return Some(index);
        }
    }
    None
}

fn is_close_angle(token: Node<'_>) -> bool {
    matches!(token.kind(), ">" | ">>")
}

fn is_segment(token: Node<'_>) -> bool {
    matches!(token.kind(), "identifier" | "self" | "super" | "crate")
}

/// The source from `first` through `last`, whitespace removed.
fn text(first: Node<'_>, last: Node<'_>, src: &[u8]) -> String {
    let bytes = src
        .get(first.start_byte()..last.end_byte())
        .unwrap_or_default();
    String::from_utf8_lossy(bytes).split_whitespace().collect()
}
