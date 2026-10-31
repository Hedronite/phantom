//! Type hints for method-call receivers (SPEC-w10 G2a, D5).
//!
//! A method call `x.m()` whose receiver is a plain identifier takes a hint
//! from the nearest binding of `x` before the call, in an enclosing block of
//! the same function. Nested `fn`s are their own scope; a closure body sees
//! its enclosing function's bindings.
//!
//! | Binding | Hint |
//! |---|---|
//! | parameter `x: T`, `&T`, `&mut T`, `Box<T>`, `Rc<T>`, `Arc<T>` | `type:T` |
//! | `let x: T = …` with the same wrappers | `type:T` |
//! | `let x = T { … }` | `type:T` |
//! | `let x = T::f(…)` or `let x = T::f(…)?` | `ctor:T::f` or `ctor?:T::f` |
//!
//! `T` is the path as written, without generic arguments, and `Self` is the
//! impl's self type. Any other binding of `x` clears the hint: a `let`
//! without a derivable type, a pattern in `match`, `if let`, `while let`,
//! `for` or a closure parameter, a tuple or struct pattern, `ref x`, a
//! local `const` or `static`, or a type that is not a path, such as
//! `impl Trait`, `dyn Trait`, a tuple or a primitive. So does a `T` that is
//! a generic parameter of the enclosing function or impl. Nothing is
//! resolved here.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::bindings::bind_pattern;
use crate::compact;

/// The smart pointers a binding's type may wrap `T` in, one level deep.
const WRAPPERS: [&str; 3] = ["Box", "Rc", "Arc"];

/// What one binding site says about the receiver's name.
enum Binding {
    /// It binds the name, with this hint, or none when the hint is cleared.
    Binds(Option<String>),
    /// It does not bind the name.
    Other,
}

/// The hint for a method call whose receiver is the identifier `receiver`.
pub(crate) fn hint(receiver: Node<'_>, src: &[u8]) -> Option<String> {
    if receiver.kind() != "identifier" {
        return None;
    }
    let name = compact(receiver, src);
    let mut child = receiver;
    while let Some(node) = child.parent() {
        let binding = match node.kind() {
            "block" => in_block(node, child, &name, src),
            "closure_expression" if is_field(node, "body", child) => node
                .child_by_field_name("parameters")
                .map_or(Binding::Other, |params| {
                    cleared_if(binds(params, &name, src))
                }),
            "for_expression" if is_field(node, "body", child) => node
                .child_by_field_name("pattern")
                .map_or(Binding::Other, |pattern| {
                    cleared_if(binds(pattern, &name, src))
                }),
            "if_expression" if !is_field(node, "alternative", child) => {
                in_condition(node, &name, src)
            }
            "while_expression" => in_condition(node, &name, src),
            "match_arm" => node
                .child_by_field_name("pattern")
                .map_or(Binding::Other, |pattern| in_arm(pattern, &name, src)),
            "function_item" => return in_parameters(node, &name, src),
            "impl_item" | "trait_item" | "mod_item" | "source_file" | "const_item"
            | "static_item" => return None,
            _ => Binding::Other,
        };
        if let Binding::Binds(hint) = binding {
            return hint;
        }
        child = node;
    }
    None
}

/// The statements of `block` before the one holding `child`, nearest first.
fn in_block(block: Node<'_>, child: Node<'_>, name: &str, src: &[u8]) -> Binding {
    let mut cursor = block.walk();
    let before: Vec<Node<'_>> = block
        .named_children(&mut cursor)
        .take_while(|statement| statement.id() != child.id())
        .collect();
    for statement in before.into_iter().rev() {
        let binding = match statement.kind() {
            "let_declaration" => in_let(statement, name, src),
            "const_item" | "static_item" => cleared_if(
                statement
                    .child_by_field_name("name")
                    .is_some_and(|item| compact(item, src) == name),
            ),
            _ => Binding::Other,
        };
        if let Binding::Binds(_) = binding {
            return binding;
        }
    }
    Binding::Other
}

/// A `let`: a plain identifier pattern takes the type or value's hint; any
/// other pattern that binds the name clears it.
fn in_let(statement: Node<'_>, name: &str, src: &[u8]) -> Binding {
    let Some(pattern) = statement.child_by_field_name("pattern") else {
        return Binding::Other;
    };
    if !is_name(pattern, name, src) {
        return cleared_if(binds(pattern, name, src));
    }
    let hint = match (
        statement.child_by_field_name("type"),
        statement.child_by_field_name("value"),
    ) {
        (Some(ty), _) => type_hint(ty, statement, src),
        (None, Some(value)) => value_hint(value, statement, src),
        (None, None) => None,
    };
    Binding::Binds(hint)
}

/// The parameters of the enclosing function, the last scope searched.
fn in_parameters(function: Node<'_>, name: &str, src: &[u8]) -> Option<String> {
    let parameters = function.child_by_field_name("parameters")?;
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if parameter.kind() != "parameter" {
            continue;
        }
        let Some(pattern) = parameter.child_by_field_name("pattern") else {
            continue;
        };
        if is_name(pattern, name, src) {
            return type_hint(parameter.child_by_field_name("type")?, function, src);
        }
        if binds(pattern, name, src) {
            return None;
        }
    }
    None
}

/// A `match` arm whose pattern binds the name; its guard binds nothing.
fn in_arm(pattern: Node<'_>, name: &str, src: &[u8]) -> Binding {
    let guard = pattern
        .child_by_field_name("condition")
        .map(|guard| guard.id());
    let mut cursor = pattern.walk();
    let bound = pattern
        .named_children(&mut cursor)
        .filter(|child| Some(child.id()) != guard)
        .any(|child| binds(child, name, src));
    cleared_if(bound)
}

/// An `if` or `while` whose condition holds a `let` binding the name.
fn in_condition(node: Node<'_>, name: &str, src: &[u8]) -> Binding {
    let Some(condition) = node.child_by_field_name("condition") else {
        return Binding::Other;
    };
    let mut stack = vec![condition];
    while let Some(part) = stack.pop() {
        match part.kind() {
            "let_condition"
                if part
                    .child_by_field_name("pattern")
                    .is_some_and(|pattern| binds(pattern, name, src)) =>
            {
                return Binding::Binds(None);
            }
            "let_chain" => {
                let mut cursor = part.walk();
                stack.extend(part.named_children(&mut cursor));
            }
            _ => {}
        }
    }
    Binding::Other
}

/// `type:T` for a path type `T`, or for `&T`, `&mut T` or one of
/// [`WRAPPERS`] around one. Only one level is unwrapped.
fn type_hint(ty: Node<'_>, at: Node<'_>, src: &[u8]) -> Option<String> {
    let inner = match ty.kind() {
        "reference_type" => {
            let inner = ty.child_by_field_name("type")?;
            if inner.kind() == "generic_type" && is_wrapper(inner, src) {
                return None;
            }
            inner
        }
        "generic_type" if is_wrapper(ty, src) => {
            let arguments = ty.child_by_field_name("type_arguments")?;
            let mut cursor = arguments.walk();
            let types: Vec<Node<'_>> = arguments.named_children(&mut cursor).collect();
            let [one] = types[..] else {
                return None;
            };
            one
        }
        _ => ty,
    };
    let path = match inner.kind() {
        "type_identifier" | "scoped_type_identifier" => inner,
        "generic_type" => inner.child_by_field_name("type")?,
        _ => return None,
    };
    Some(format!("type:{}", type_path(path, at, src)?))
}

/// `type:T` for `T { .. }`, `ctor:T::f` for `T::f(..)` and `ctor?:T::f` for
/// `T::f(..)?`.
fn value_hint(value: Node<'_>, at: Node<'_>, src: &[u8]) -> Option<String> {
    match value.kind() {
        "struct_expression" => {
            let name = value.child_by_field_name("name")?;
            let path = match name.kind() {
                "generic_type_with_turbofish" => name.child_by_field_name("type")?,
                _ => name,
            };
            Some(format!("type:{}", type_path(path, at, src)?))
        }
        "call_expression" => Some(format!("ctor:{}", ctor(value, at, src)?)),
        "try_expression" => {
            let call = value.named_child(0)?;
            (call.kind() == "call_expression")
                .then(|| ctor(call, at, src))
                .flatten()
                .map(|path| format!("ctor?:{path}"))
        }
        _ => None,
    }
}

/// `T::f` for a call whose callee is a path of two or more segments.
fn ctor(call: Node<'_>, at: Node<'_>, src: &[u8]) -> Option<String> {
    let mut function = call.child_by_field_name("function")?;
    if function.kind() == "generic_function" {
        function = function.child_by_field_name("function")?;
    }
    if function.kind() != "scoped_identifier" {
        return None;
    }
    let ty = function.child_by_field_name("path")?;
    let name = compact(function.child_by_field_name("name")?, src);
    Some(format!("{}::{name}", type_path(ty, at, src)?))
}

/// A type path as written, generic arguments removed, with `Self` replaced
/// by the impl's self type. None for `_`, a qualified path, a path under
/// `Self` such as `Self::Item`, a generic parameter of the enclosing
/// function or impl, or `Self` outside an impl.
fn type_path(path: Node<'_>, at: Node<'_>, src: &[u8]) -> Option<String> {
    let plain = plain(&compact(path, src))?;
    if plain == "_" {
        return None;
    }
    if plain == "Self" {
        return self_type(at, src);
    }
    if plain.starts_with("Self::") {
        return None;
    }
    if !plain.contains("::") && generic_parameters(at, src).contains(&plain) {
        return None;
    }
    Some(plain)
}

/// The self type of the impl whose method holds `at`, as a plain path.
fn self_type(at: Node<'_>, src: &[u8]) -> Option<String> {
    let function = enclosing(at, "function_item")?;
    let owner = function
        .parent()
        .filter(|parent| parent.kind() == "declaration_list")?
        .parent()
        .filter(|owner| owner.kind() == "impl_item")?;
    let ty = owner.child_by_field_name("type")?;
    let path = match ty.kind() {
        "type_identifier" | "scoped_type_identifier" => ty,
        "generic_type" => ty.child_by_field_name("type")?,
        _ => return None,
    };
    plain(&compact(path, src))
}

/// The type parameter names of the function holding `at` and of its impl.
fn generic_parameters(at: Node<'_>, src: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    let function = enclosing(at, "function_item");
    let owner = function
        .and_then(|function| function.parent())
        .filter(|parent| parent.kind() == "declaration_list")
        .and_then(|list| list.parent());
    for node in [function, owner].into_iter().flatten() {
        let Some(parameters) = node.child_by_field_name("type_parameters") else {
            continue;
        };
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            if parameter.kind() == "type_parameter"
                && let Some(name) = parameter.child_by_field_name("name")
            {
                names.insert(compact(name, src));
            }
        }
    }
    names
}

/// The nearest ancestor of `node`, or `node` itself, of `kind`.
fn enclosing<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut current = Some(node);
    while let Some(candidate) = current {
        if candidate.kind() == kind {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// True for `Box<..>`, `Rc<..>` or `Arc<..>`, bare or under `std`, `alloc`
/// or `core`.
fn is_wrapper(ty: Node<'_>, src: &[u8]) -> bool {
    let Some(path) = ty.child_by_field_name("type") else {
        return false;
    };
    let Some(plain) = plain(&compact(path, src)) else {
        return false;
    };
    let mut segments: Vec<&str> = plain.split("::").collect();
    let last = segments.pop().unwrap_or_default();
    WRAPPERS.contains(&last)
        && segments
            .first()
            .is_none_or(|first| ["std", "alloc", "core"].contains(first))
}

/// `text` with generic arguments removed, or none for a path that starts at
/// a qualified type such as `<T as Trait>`.
fn plain(text: &str) -> Option<String> {
    if text.starts_with('<') {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut depth = 0_usize;
    for ch in text.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let out = out.replace("::::", "::");
    let out = out.strip_suffix("::").unwrap_or(&out).to_owned();
    (!out.is_empty()).then_some(out)
}

fn is_field(node: Node<'_>, field: &str, child: Node<'_>) -> bool {
    node.child_by_field_name(field)
        .is_some_and(|value| value.id() == child.id())
}

fn is_name(pattern: Node<'_>, name: &str, src: &[u8]) -> bool {
    pattern.kind() == "identifier" && compact(pattern, src) == name
}

fn binds(pattern: Node<'_>, name: &str, src: &[u8]) -> bool {
    let mut names = HashSet::new();
    bind_pattern(pattern, src, &mut names);
    names.contains(name)
}

fn cleared_if(bound: bool) -> Binding {
    if bound {
        Binding::Binds(None)
    } else {
        Binding::Other
    }
}
