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
//!
//! The walk for one call stops after [`MAX_HINT_VISITS`] visits, and the
//! call then gets no hint (SPEC-w11 G0b).

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::bindings::bind_pattern;
use crate::compact;

/// The most nodes the walk for one call visits (SPEC-w11 G0b). Each
/// ancestor of the receiver counts one, as does each statement of an
/// enclosing block before the call, nearest first, and each parameter of
/// the enclosing function. Past it the call gets no hint, so a long
/// function costs at most this much per call rather than its length.
pub const MAX_HINT_VISITS: usize = 4096;

/// The smart pointers a binding's type may wrap `T` in, one level deep.
const WRAPPERS: [&str; 3] = ["Box", "Rc", "Arc"];

/// What one binding site says about the receiver's name.
enum Binding {
    /// It binds the name, with this hint, or none when the hint is cleared.
    Binds(Option<String>),
    /// It does not bind the name.
    Other,
    /// The walk ran past [`MAX_HINT_VISITS`].
    Spent,
}

/// The visits left to one call's walk.
struct Budget(usize);

impl Budget {
    /// Counts one visit: false once the walk is past [`MAX_HINT_VISITS`].
    fn visit(&mut self) -> bool {
        self.0 += 1;
        self.0 <= MAX_HINT_VISITS
    }
}

/// The statements of each block a walk has entered, read once per file so
/// that a visit to a statement is a name comparison.
#[derive(Default)]
pub(crate) struct Blocks<'tree> {
    seen: HashMap<usize, Block<'tree>>,
}

/// One block's named children, in order.
struct Block<'tree> {
    statements: Vec<Statement<'tree>>,
    /// Each statement's position in `statements`, by node id.
    position: HashMap<usize, usize>,
}

/// What one statement of a block binds.
struct Statement<'tree> {
    node: Node<'tree>,
    /// The name a `let` with a plain identifier pattern binds.
    plain: Option<String>,
    /// The names it binds that clear a hint: those of any other `let`
    /// pattern, or a local `const` or `static`'s name.
    clears: HashSet<String>,
}

impl<'tree> Blocks<'tree> {
    fn block(&mut self, block: Node<'tree>, src: &[u8]) -> &Block<'tree> {
        self.seen.entry(block.id()).or_insert_with(|| {
            let mut cursor = block.walk();
            let statements: Vec<Statement<'tree>> = block
                .named_children(&mut cursor)
                .map(|node| statement(node, src))
                .collect();
            let position = statements
                .iter()
                .enumerate()
                .map(|(at, statement)| (statement.node.id(), at))
                .collect();
            Block {
                statements,
                position,
            }
        })
    }
}

fn statement<'tree>(node: Node<'tree>, src: &[u8]) -> Statement<'tree> {
    let mut plain = None;
    let mut clears = HashSet::new();
    match node.kind() {
        "let_declaration" => match node.child_by_field_name("pattern") {
            Some(pattern) if pattern.kind() == "identifier" => {
                plain = Some(compact(pattern, src));
            }
            Some(pattern) => bind_pattern(pattern, src, &mut clears),
            None => {}
        },
        "const_item" | "static_item" => {
            if let Some(item) = node.child_by_field_name("name") {
                clears.insert(compact(item, src));
            }
        }
        _ => {}
    }
    Statement {
        node,
        plain,
        clears,
    }
}

/// The hint for a method call whose receiver is the identifier `receiver`.
pub(crate) fn hint<'tree>(
    receiver: Node<'tree>,
    src: &[u8],
    blocks: &mut Blocks<'tree>,
) -> Option<String> {
    if receiver.kind() != "identifier" {
        return None;
    }
    let name = compact(receiver, src);
    let mut budget = Budget(0);
    let mut child = receiver;
    while let Some(node) = child.parent() {
        if !budget.visit() {
            return None;
        }
        let binding = match node.kind() {
            "block" => in_block(blocks.block(node, src), child, &name, src, &mut budget),
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
            "function_item" => return in_parameters(node, &name, src, &mut budget),
            "impl_item" | "trait_item" | "mod_item" | "source_file" | "const_item"
            | "static_item" => return None,
            _ => Binding::Other,
        };
        match binding {
            Binding::Binds(hint) => return hint,
            Binding::Spent => return None,
            Binding::Other => child = node,
        }
    }
    None
}

/// The statements of `block` before the one holding `child`, nearest first.
fn in_block(
    block: &Block<'_>,
    child: Node<'_>,
    name: &str,
    src: &[u8],
    budget: &mut Budget,
) -> Binding {
    let before = block
        .position
        .get(&child.id())
        .copied()
        .unwrap_or(block.statements.len());
    for statement in block.statements[..before].iter().rev() {
        if !budget.visit() {
            return Binding::Spent;
        }
        if statement.plain.as_deref() == Some(name) {
            return Binding::Binds(let_hint(statement.node, src));
        }
        if statement.clears.contains(name) {
            return Binding::Binds(None);
        }
    }
    Binding::Other
}

/// The hint of a `let` whose pattern is a plain identifier: from its type,
/// or else from its value.
fn let_hint(statement: Node<'_>, src: &[u8]) -> Option<String> {
    match (
        statement.child_by_field_name("type"),
        statement.child_by_field_name("value"),
    ) {
        (Some(ty), _) => type_hint(ty, statement, src),
        (None, Some(value)) => value_hint(value, statement, src),
        (None, None) => None,
    }
}

/// The parameters of the enclosing function, the last scope searched.
fn in_parameters(
    function: Node<'_>,
    name: &str,
    src: &[u8],
    budget: &mut Budget,
) -> Option<String> {
    let parameters = function.child_by_field_name("parameters")?;
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !budget.visit() {
            return None;
        }
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
