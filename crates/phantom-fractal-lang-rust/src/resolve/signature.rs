//! Reading stored signatures for [`super::lookup`]: the return type, the
//! `Ok` or `Some` type inside it, and the type parameters in scope
//! (SPEC-w10 G2a, SPEC-w12 G2a).

use std::collections::HashSet;

use phantom_fractal_lang::Symbol;

/// The return type in a stored signature, whitespace removed: the text
/// after the first `->` outside brackets, up to a `where` clause. None
/// without one.
pub(super) fn return_type(signature: &str) -> Option<String> {
    let mut depth = 0_usize;
    let mut chars = signature.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '-' if chars.peek().is_some_and(|(_, next)| *next == '>') => {
                chars.next();
                if depth == 0 {
                    let rest = &signature[at + 2..];
                    let rest = rest.split(" where ").next().unwrap_or(rest);
                    let text: String = rest.split_whitespace().collect();
                    return (!text.is_empty()).then_some(text);
                }
            }
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// `X` for `Result<X, ..>` or `Option<X>`, by the outer type's last path
/// segment.
pub(super) fn ok_type(returned: &str) -> Option<String> {
    let (outer, inner) = returned.split_once('<')?;
    let inner = inner.strip_suffix('>')?;
    let mut depth = 0_usize;
    let mut arguments = vec![String::new()];
    for ch in inner.chars() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = arguments.last_mut() {
            last.push(ch);
        }
    }
    match (last_segment(outer), &arguments[..]) {
        ("Result", [ok, ..]) | ("Option", [ok]) if !ok.is_empty() => Some(ok.clone()),
        _ => None,
    }
}

/// The first path segment of a type, generic arguments aside.
pub(super) fn first_segment(ty: &str) -> &str {
    let end = ty.find(['<', ':']).unwrap_or(ty.len());
    &ty[..end]
}

/// The type parameter names of a stored fn or method: those its signature
/// declares after its name, and for a method those its impl's self type
/// takes, as `Ledger<T>` takes `T`. Lifetimes and consts are left out.
pub(super) fn generic_parameters(function: &Symbol) -> HashSet<String> {
    let mut out = HashSet::new();
    let declared = function.signature.as_deref().and_then(|signature| {
        let after = signature.split_once("fn ")?.1;
        let after = after.trim_start_matches(|ch: char| ch.is_alphanumeric() || ch == '_');
        after.starts_with('<').then_some(after)
    });
    let taken = function
        .owner_path
        .as_deref()
        .and_then(|owner| owner.find('<').map(|at| &owner[at..]));
    for list in [declared, taken].into_iter().flatten() {
        out.extend(angle_list(list).into_iter().filter_map(|parameter| {
            let name = parameter
                .split([':', '='])
                .next()
                .unwrap_or_default()
                .trim();
            let name = name.strip_prefix("const ").unwrap_or(name).trim();
            (!name.is_empty() && !name.starts_with('\'')).then(|| name.to_owned())
        }));
    }
    out
}

/// The comma-separated items of the `<…>` list `text` starts with.
fn angle_list(text: &str) -> Vec<String> {
    let mut depth = 0_usize;
    let mut items = vec![String::new()];
    for ch in text.chars() {
        match ch {
            '<' if depth == 0 => {
                depth = 1;
                continue;
            }
            '<' | '(' | '[' => depth += 1,
            '>' if depth == 1 => break,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 1 => {
                items.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = items.last_mut() {
            last.push(ch);
        }
    }
    items
}

/// The last segment of a `::` path.
fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}
