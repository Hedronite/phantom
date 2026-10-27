//! Names a function binds locally, for the conservative `shadowed` flag
//! (SPEC-w3 G0a).
//!
//! Every pattern in the function counts, wherever it sits: parameters,
//! `let`, closure parameters, `match` arms, `for`, `if let`, and `while let`.
//! So do the names of items and `use` leaves declared in the body. Nested
//! functions are their own scope and are not entered.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::{compact, imports};

/// Every name `function` binds in its parameters or body.
pub(crate) fn collect(function: Node<'_>, src: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut stack: Vec<Node<'_>> = ["parameters", "body"]
        .into_iter()
        .filter_map(|field| function.child_by_field_name(field))
        .collect();
    while let Some(node) = stack.pop() {
        match node.kind() {
            "parameter" | "let_declaration" | "for_expression" | "let_condition" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    bind_pattern(pattern, src, &mut names);
                }
            }
            "closure_parameters" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if child.kind() != "parameter" {
                        bind_pattern(child, src, &mut names);
                    }
                }
            }
            "match_pattern" => {
                let guard = node
                    .child_by_field_name("condition")
                    .map(|guard| guard.id());
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if Some(child.id()) != guard {
                        bind_pattern(child, src, &mut names);
                    }
                }
            }
            "function_item" | "struct_item" | "enum_item" | "union_item" | "trait_item"
            | "const_item" | "static_item" | "type_item" | "mod_item" => {
                if let Some(name) = node.child_by_field_name("name") {
                    names.insert(compact(name, src));
                }
                continue;
            }
            "use_declaration" => {
                bind_use(node, src, &mut names);
                continue;
            }
            "impl_item" | "macro_definition" | "token_tree" => continue,
            _ => {}
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    names
}

/// Adds the identifiers a pattern binds. Paths and the type of a struct or
/// tuple-struct pattern bind nothing. A bare identifier always counts, even
/// when it names a unit variant or a constant.
fn bind_pattern(pattern: Node<'_>, src: &[u8], names: &mut HashSet<String>) {
    let mut stack = vec![pattern];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_field_identifier" => {
                names.insert(compact(node, src));
            }
            "scoped_identifier"
            | "scoped_type_identifier"
            | "generic_type"
            | "type_identifier"
            | "macro_invocation" => {}
            _ => {
                let skip = node.child_by_field_name("type").map(|path| path.id());
                let mut cursor = node.walk();
                stack.extend(
                    node.named_children(&mut cursor)
                        .filter(|child| Some(child.id()) != skip),
                );
            }
        }
    }
}

/// Adds the name each non-glob leaf of a local `use` binds.
fn bind_use(declaration: Node<'_>, src: &[u8], names: &mut HashSet<String>) {
    let Some(argument) = declaration.child_by_field_name("argument") else {
        return;
    };
    let mut leaves = Vec::new();
    imports::flatten(
        argument,
        src,
        &phantom_fractal_lang::Visibility::Private,
        0,
        &mut leaves,
    );
    for leaf in leaves.into_iter().filter(|leaf| !leaf.glob) {
        let name = match leaf.alias {
            Some(alias) => alias,
            None => leaf.path.rsplit("::").next().unwrap_or_default().to_owned(),
        };
        names.insert(name);
    }
}
