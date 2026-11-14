//! Receiver hints from return types and same-file fields (SPEC-w12 G2a,
//! D6).
//!
//! | Receiver | Hint |
//! |---|---|
//! | `let x = f(…)` or `let x = f(…)?`, then `x.m()` | `ret:f` or `ret?:f` |
//! | `f(…).m()` or `f(…)?.m()` | `ret:f` or `ret?:f` |
//! | `T::f(…).m()` or `T::f(…)?.m()` | `ctor:T::f` or `ctor?:T::f` |
//! | `self.f.m()`, `Self` a struct of this file with a named field `f: T` | `type:T` |
//!
//! `f` is a single identifier, generic arguments aside; a name the
//! enclosing function binds, such as a closure, gives no hint. A call
//! receiver is one level only: `x.f().m()` and `a.b.m()` give none. A field
//! type follows the W10 wrappers, and a type that is a generic parameter of
//! the struct, the function or the impl gives none, as does a field of a
//! struct defined in another file or a self type that two structs of this
//! file share. Nothing is resolved here.

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::receivers::{ctor, enclosing, self_type, type_hint};
use crate::{bindings, compact};

/// What the hints of one file read once: the names each function binds,
/// and the file's structs by name.
#[derive(Default)]
pub(crate) struct Cures<'tree> {
    bound: HashMap<usize, HashSet<String>>,
    structs: Option<HashMap<String, Vec<Node<'tree>>>>,
}

impl<'tree> Cures<'tree> {
    /// True when the function holding `at` binds `name`.
    pub(crate) fn binds(&mut self, at: Node<'tree>, name: &str, src: &[u8]) -> bool {
        let Some(function) = enclosing(at, "function_item") else {
            return false;
        };
        self.bound
            .entry(function.id())
            .or_insert_with(|| bindings::collect(function, src))
            .contains(name)
    }

    /// The struct items of the file holding `at` named `name`.
    fn structs(&mut self, at: Node<'tree>, src: &[u8]) -> &HashMap<String, Vec<Node<'tree>>> {
        self.structs.get_or_insert_with(|| {
            let mut root = at;
            while let Some(parent) = root.parent() {
                root = parent;
            }
            let mut out: HashMap<String, Vec<Node<'tree>>> = HashMap::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if node.kind() == "struct_item"
                    && let Some(name) = node.child_by_field_name("name")
                {
                    out.entry(compact(name, src)).or_default().push(node);
                }
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            }
            out
        })
    }
}

/// `ret:f` or `ctor:T::f` for a path call `value`, with `?` when `value` is
/// that call followed by `?`. `at` is where the hint is read.
pub(crate) fn call_hint<'tree>(
    value: Node<'tree>,
    at: Node<'tree>,
    src: &[u8],
    cures: &mut Cures<'tree>,
) -> Option<String> {
    let (call, mark) = match value.kind() {
        "call_expression" => (value, ""),
        "try_expression" => (
            value
                .named_child(0)
                .filter(|inner| inner.kind() == "call_expression")?,
            "?",
        ),
        _ => return None,
    };
    if let Some(path) = ctor(call, at, src) {
        return Some(format!("ctor{mark}:{path}"));
    }
    let name = called_name(call, src)?;
    (!cures.binds(at, &name, src)).then(|| format!("ret{mark}:{name}"))
}

/// The callee of `call` when it is one identifier, generic arguments aside.
fn called_name(call: Node<'_>, src: &[u8]) -> Option<String> {
    let mut function = call.child_by_field_name("function")?;
    if function.kind() == "generic_function" {
        function = function.child_by_field_name("function")?;
    }
    (function.kind() == "identifier").then(|| compact(function, src))
}

/// `type:T` for `self.f`, from the named field `f: T` of the one struct of
/// this file that the impl's self type names.
pub(crate) fn field_hint<'tree>(
    field_expression: Node<'tree>,
    src: &[u8],
    cures: &mut Cures<'tree>,
) -> Option<String> {
    let value = field_expression.child_by_field_name("value")?;
    let field = field_expression.child_by_field_name("field")?;
    if value.kind() != "self" || field.kind() != "field_identifier" {
        return None;
    }
    // A scoped self type, such as `a::S`, names no struct by its bare name.
    let owner = self_type(field_expression, src)?;
    let [item] = cures.structs(field_expression, src).get(&owner)?.as_slice() else {
        return None;
    };
    let ty = field_type(*item, &compact(field, src), src)?;
    let hint = type_hint(ty, field_expression, src)?;
    let generic = hint
        .strip_prefix("type:")
        .is_some_and(|path| type_parameters(*item, src).contains(path));
    (!generic).then_some(hint)
}

/// The type of the named field `name` of the struct `item`.
fn field_type<'tree>(item: Node<'tree>, name: &str, src: &[u8]) -> Option<Node<'tree>> {
    let body = item.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let found = body
        .named_children(&mut cursor)
        .filter(|field| field.kind() == "field_declaration")
        .find(|field| {
            field
                .child_by_field_name("name")
                .is_some_and(|field_name| compact(field_name, src) == name)
        })?;
    found.child_by_field_name("type")
}

/// The type parameter names of the struct `item`.
fn type_parameters(item: Node<'_>, src: &[u8]) -> HashSet<String> {
    let Some(parameters) = item.child_by_field_name("type_parameters") else {
        return HashSet::new();
    };
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter(|parameter| parameter.kind() == "type_parameter")
        .filter_map(|parameter| parameter.child_by_field_name("name"))
        .map(|name| compact(name, src))
        .collect()
}
