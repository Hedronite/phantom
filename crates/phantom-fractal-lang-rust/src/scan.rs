//! One pass over the syntax tree for call references, methods, and module
//! declarations (SPEC-w3 G0a, G0b).
//!
//! The pass keeps an explicit stack, so a deeply nested expression cannot
//! overflow the thread stack. Macros are never expanded: the tree walk does
//! not enter a token tree, and a macro invocation's tokens are only read for
//! call-shaped names (SPEC-w5 G1a).

use std::collections::HashSet;
use std::rc::Rc;

use phantom_fractal_lang::{Caller, Extract, Method, ModDecl, RefKind, Reference, Visibility};
use tree_sitter::Node;

use crate::macro_args::call_shapes;
use crate::{at_module_scope, attrs, bindings, compact, detail, receivers, start_line, visibility};

/// What a node inherits from its ancestors.
struct Context {
    /// The inline-module path.
    scope: String,
    /// The innermost enclosing function.
    function: Option<Function>,
}

struct Function {
    caller: Caller,
    bindings: HashSet<String>,
}

/// Appends refs, methods, and module declarations under `root` to `out`.
pub(crate) fn scan(root: Node<'_>, src: &[u8], out: &mut Extract) {
    let top = Rc::new(Context {
        scope: String::new(),
        function: None,
    });
    let mut stack = vec![(root, top)];
    while let Some((node, context)) = stack.pop() {
        if node.kind() == "token_tree" {
            continue;
        }
        let context = visit(node, src, context, out);
        let mut cursor = node.walk();
        let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push((child, Rc::clone(&context)));
        }
    }
    out.refs
        .sort_by_key(|reference| (reference.line, reference.col));
}

/// Records what `node` is, and returns the context its children inherit.
fn visit(node: Node<'_>, src: &[u8], context: Rc<Context>, out: &mut Extract) -> Rc<Context> {
    match node.kind() {
        "mod_item" => {
            let name = node
                .child_by_field_name("name")
                .map(|name| compact(name, src))
                .unwrap_or_default();
            let body = node.child_by_field_name("body");
            if at_module_scope(node) {
                out.mod_decls.push(ModDecl {
                    name: name.clone(),
                    scope: context.scope.clone(),
                    inline: body.is_some(),
                    path: path_attribute(node, src),
                    line: start_line(node),
                    cfg_test: attrs::cfg_test(node, src),
                });
            }
            if body.is_none() {
                return context;
            }
            let scope = if context.scope.is_empty() {
                name
            } else {
                format!("{}::{name}", context.scope)
            };
            Rc::new(Context {
                scope,
                function: None,
            })
        }
        "function_item" => {
            if let Some(method) = method(node, src, &context.scope) {
                out.methods.push(method);
            }
            Rc::new(Context {
                scope: context.scope.clone(),
                function: Some(Function {
                    caller: caller(node, src),
                    bindings: bindings::collect(node, src),
                }),
            })
        }
        "call_expression" => {
            if let Some(reference) = call(node, src, &context) {
                out.refs.push(reference);
            }
            context
        }
        "macro_invocation" => {
            if let Some(reference) = macro_call(node, src, &context) {
                out.refs.push(reference);
            }
            for shape in call_shapes(node, src) {
                let callee = Callee {
                    kind: RefKind::MacroArg,
                    text: shape.text,
                    last_segment: shape.last_segment,
                    at: shape.at,
                    receiver: None,
                    receiver_type: None,
                    single_segment: false,
                };
                out.refs.push(reference(callee, false, &context));
            }
            context
        }
        _ => context,
    }
}

/// The callee shapes a call reference is built from.
struct Callee<'tree> {
    kind: RefKind,
    text: String,
    last_segment: String,
    /// The node whose start is the reference's position.
    at: Node<'tree>,
    receiver: Option<String>,
    /// See [`Reference::receiver_type`].
    receiver_type: Option<String>,
    single_segment: bool,
}

/// A `Call` or `MethodCall` reference, or none for a callee that is neither
/// a path nor a field: `(f)()`, `f()()`, `fns[0]()`.
fn call(node: Node<'_>, src: &[u8], context: &Context) -> Option<Reference> {
    let function = node.child_by_field_name("function")?;
    let callee = match function.kind() {
        "identifier" | "scoped_identifier" => path_callee(function, function, src),
        "field_expression" => method_callee(function, None, src),
        "generic_function" => {
            let inner = function.child_by_field_name("function")?;
            match inner.kind() {
                "identifier" | "scoped_identifier" => path_callee(function, inner, src),
                "field_expression" => {
                    method_callee(inner, function.child_by_field_name("type_arguments"), src)
                }
                _ => None,
            }
        }
        _ => None,
    }?;
    let shadowed = callee.kind == RefKind::Call
        && callee.single_segment
        && context
            .function
            .as_ref()
            .is_some_and(|function| function.bindings.contains(&callee.last_segment));
    Some(reference(callee, shadowed, context))
}

/// A path callee. `written` is the whole callee with any generic arguments;
/// `path` is the identifier or scoped identifier inside it.
fn path_callee<'tree>(
    written: Node<'tree>,
    path: Node<'tree>,
    src: &[u8],
) -> Option<Callee<'tree>> {
    let (last, single_segment) = match path.kind() {
        "identifier" => (path, true),
        _ => (path.child_by_field_name("name")?, false),
    };
    Some(Callee {
        kind: RefKind::Call,
        text: compact(written, src),
        last_segment: compact(last, src),
        at: written,
        receiver: None,
        receiver_type: None,
        single_segment,
    })
}

/// A method callee: `x.m` or, with `type_arguments`, `x.m::<T>`.
fn method_callee<'tree>(
    field_expression: Node<'tree>,
    type_arguments: Option<Node<'tree>>,
    src: &[u8],
) -> Option<Callee<'tree>> {
    let field = field_expression.child_by_field_name("field")?;
    let name = compact(field, src);
    let text = match type_arguments {
        Some(arguments) => format!("{name}::{}", compact(arguments, src)),
        None => name.clone(),
    };
    let value = field_expression.child_by_field_name("value");
    Some(Callee {
        kind: RefKind::MethodCall,
        text,
        last_segment: name,
        at: field,
        receiver: value.map(|value| compact(value, src)),
        receiver_type: value.and_then(|value| receivers::hint(value, src)),
        single_segment: false,
    })
}

fn macro_call(node: Node<'_>, src: &[u8], context: &Context) -> Option<Reference> {
    let path = node.child_by_field_name("macro")?;
    let mut callee = path_callee(path, path, src)?;
    callee.kind = RefKind::Macro;
    Some(reference(callee, false, context))
}

fn reference(callee: Callee<'_>, shadowed: bool, context: &Context) -> Reference {
    let position = callee.at.start_position();
    Reference {
        kind: callee.kind,
        text: callee.text,
        last_segment: callee.last_segment,
        line: one_based(position.row),
        col: one_based(position.column),
        scope: context.scope.clone(),
        from: context
            .function
            .as_ref()
            .map(|function| function.caller.clone()),
        receiver: callee.receiver,
        receiver_type: callee.receiver_type,
        shadowed,
    }
}

fn one_based(index: usize) -> u32 {
    u32::try_from(index).map_or(u32::MAX, |index| index.saturating_add(1))
}

/// The function's name, with the `impl` self type or trait it sits in.
fn caller(function: Node<'_>, src: &[u8]) -> Caller {
    let name = function
        .child_by_field_name("name")
        .map(|name| compact(name, src))
        .unwrap_or_default();
    let owner = container(function).and_then(|owner| match owner.kind() {
        "impl_item" => owner
            .child_by_field_name("type")
            .map(|ty| owner_name(ty, src)),
        "trait_item" => owner
            .child_by_field_name("name")
            .map(|name| compact(name, src)),
        _ => None,
    });
    Caller {
        name,
        owner,
        start_line: start_line(function),
    }
}

/// The item whose body directly holds `function`, if any.
fn container(function: Node<'_>) -> Option<Node<'_>> {
    function
        .parent()
        .filter(|parent| parent.kind() == "declaration_list")?
        .parent()
}

/// A method record for a `fn` directly inside an `impl` at module scope.
fn method(function: Node<'_>, src: &[u8], scope: &str) -> Option<Method> {
    let owner = container(function).filter(|owner| owner.kind() == "impl_item")?;
    if !at_module_scope(owner) {
        return None;
    }
    let self_type = owner.child_by_field_name("type")?;
    let mut cursor = function.walk();
    let visibility = function
        .children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .map_or(Visibility::Private, |modifier| visibility(modifier, src));
    let detail = detail::detail(function, src);
    Some(Method {
        owner: owner_name(self_type, src),
        owner_path: compact(self_type, src),
        trait_name: owner
            .child_by_field_name("trait")
            .map(|name| compact(name, src)),
        name: compact(function.child_by_field_name("name")?, src),
        visibility,
        scope: scope.to_owned(),
        start_line: start_line(function),
        end_line: detail.end_line,
        signature: detail.signature,
        doc_summary: detail.doc_summary,
        is_test: attrs::is_test(function, src),
    })
}

/// A self type's last path segment without generics: `a::Ledger<T>` and
/// `&Ledger` give `Ledger`. Other type shapes come back as written.
fn owner_name(self_type: Node<'_>, src: &[u8]) -> String {
    let mut node = self_type;
    loop {
        let next = match node.kind() {
            "generic_type" | "reference_type" | "pointer_type" => node.child_by_field_name("type"),
            "scoped_type_identifier" => node.child_by_field_name("name"),
            _ => None,
        };
        match next {
            Some(next) => node = next,
            None => return compact(node, src),
        }
    }
}

/// The value of a `#[path = "..."]` outer attribute on a `mod` item.
fn path_attribute(module: Node<'_>, src: &[u8]) -> Option<String> {
    attrs::outer(module)
        .into_iter()
        .find_map(|attribute| path_value(attribute, src))
}

fn path_value(attribute: Node<'_>, src: &[u8]) -> Option<String> {
    let name = attribute.named_child(0)?;
    if name.kind() != "identifier" || compact(name, src) != "path" {
        return None;
    }
    let value = attribute.child_by_field_name("value")?;
    let mut cursor = value.walk();
    let content = value
        .named_children(&mut cursor)
        .find(|child| child.kind() == "string_content");
    Some(content.map_or_else(String::new, |content| {
        content.utf8_text(src).unwrap_or_default().to_owned()
    }))
}
