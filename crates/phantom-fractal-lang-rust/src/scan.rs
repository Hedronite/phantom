//! One pass over the syntax tree for call references, methods, and module
//! declarations (SPEC-w3 G0a, G0b).
//!
//! The pass keeps an explicit stack, so a deeply nested expression cannot
//! overflow the thread stack. Macros are never expanded: the tree walk does
//! not enter a token tree, and a macro invocation's tokens are only read for
//! call-shaped names (SPEC-w5 G1a). Those names keep their receiver, its
//! hint and their shadowing as a call would (SPEC-w12 G1b).

use std::collections::HashSet;
use std::rc::Rc;

use phantom_fractal_lang::{Caller, Extract, Method, ModDecl, RefKind, Reference, Visibility};
use tree_sitter::Node;

use crate::macro_args::{CallShape, Receiver, call_shapes};
use crate::receivers::Blocks;
use crate::{
    Placement, attrs, bindings, compact, detail, join_scope, receivers, start_line, visibility,
};

/// What a node inherits from its ancestors.
struct Context {
    /// The inline-module path.
    scope: Rc<String>,
    /// True inside a `#[cfg(test)]` inline module.
    in_cfg_test: bool,
    /// Current declaration placement within module/type/body hierarchy.
    placement: Placement,
    /// Direct enclosing `impl` or `trait` when inside its `declaration_list`.
    container: Option<Container>,
    /// The innermost enclosing function.
    function: Option<Rc<Function>>,
}

#[derive(Clone)]
enum Container {
    Impl {
        owner: String,
        owner_path: String,
        trait_name: Option<String>,
    },
    Trait(String),
}

struct Function {
    caller: Caller,
    bindings: HashSet<String>,
}

/// Appends refs, methods, and module declarations under `root` to `out`.
pub(crate) fn scan(root: Node<'_>, src: &[u8], out: &mut Extract) {
    let mut blocks = Blocks::default();
    let top = Rc::new(Context {
        scope: Rc::new(String::new()),
        in_cfg_test: false,
        placement: Placement::Module,
        container: None,
        function: None,
    });
    let mut stack = vec![(root, top)];
    while let Some((node, context)) = stack.pop() {
        if node.kind() == "token_tree" {
            continue;
        }
        let context = visit(node, src, context, &mut blocks, out);
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
fn visit<'tree>(
    node: Node<'tree>,
    src: &[u8],
    context: Rc<Context>,
    blocks: &mut Blocks<'tree>,
    out: &mut Extract,
) -> Rc<Context> {
    match node.kind() {
        "source_file" | "ERROR" => context,
        "mod_item" => visit_mod(node, src, context, out),
        "impl_item" => visit_impl(node, src, &context),
        "trait_item" => visit_trait(node, src, &context),
        "declaration_list" => visit_decl_list(&context),
        "function_item" => visit_fn(node, src, &context, out),
        "call_expression" => visit_call(node, src, context, blocks, out),
        "macro_invocation" => visit_macro(node, src, context, blocks, out),
        _ => clear_direct(context),
    }
}

fn clear_direct(context: Rc<Context>) -> Rc<Context> {
    if context.placement == Placement::Body {
        return context;
    }
    Rc::new(Context {
        scope: Rc::clone(&context.scope),
        in_cfg_test: context.in_cfg_test,
        placement: Placement::Body,
        container: None,
        function: context.function.clone(),
    })
}

fn visit_mod(node: Node<'_>, src: &[u8], context: Rc<Context>, out: &mut Extract) -> Rc<Context> {
    let name = node
        .child_by_field_name("name")
        .map(|name| compact(name, src))
        .unwrap_or_default();
    let body = node.child_by_field_name("body");
    let mod_cfg_test = attrs::cfg_test(node, src);
    let at_module = context.placement == Placement::Module;
    if at_module {
        out.mod_decls.push(ModDecl {
            name: name.clone(),
            scope: (*context.scope).clone(),
            inline: body.is_some(),
            path: path_attribute(node, src),
            line: start_line(node),
            cfg_test: mod_cfg_test,
        });
    }
    if body.is_none() {
        return clear_direct(context);
    }
    Rc::new(Context {
        scope: Rc::new(join_scope(&context.scope, &name)),
        in_cfg_test: context.in_cfg_test || mod_cfg_test,
        placement: if at_module {
            Placement::ModHeader
        } else {
            Placement::Body
        },
        container: None,
        function: None,
    })
}

fn visit_impl(node: Node<'_>, src: &[u8], context: &Context) -> Rc<Context> {
    let container = if context.placement == Placement::Module {
        node.child_by_field_name("type").map(|ty| Container::Impl {
            owner: owner_name(ty, src),
            owner_path: compact(ty, src),
            trait_name: node
                .child_by_field_name("trait")
                .map(|name| compact(name, src)),
        })
    } else {
        None
    };
    Rc::new(Context {
        scope: Rc::clone(&context.scope),
        in_cfg_test: context.in_cfg_test,
        placement: Placement::Body,
        container,
        function: None,
    })
}

fn visit_trait(node: Node<'_>, src: &[u8], context: &Context) -> Rc<Context> {
    let container = node
        .child_by_field_name("name")
        .map(|name| Container::Trait(compact(name, src)));
    Rc::new(Context {
        scope: Rc::clone(&context.scope),
        in_cfg_test: context.in_cfg_test,
        placement: Placement::Body,
        container,
        function: None,
    })
}

fn visit_decl_list(context: &Context) -> Rc<Context> {
    let placement = if context.placement == Placement::ModHeader {
        Placement::Module
    } else {
        Placement::TypeDeclList
    };
    Rc::new(Context {
        scope: Rc::clone(&context.scope),
        in_cfg_test: context.in_cfg_test,
        placement,
        container: context.container.clone(),
        function: None,
    })
}

fn visit_fn(node: Node<'_>, src: &[u8], context: &Context, out: &mut Extract) -> Rc<Context> {
    if let Some(method) = method(node, src, context) {
        out.methods.push(method);
    }
    Rc::new(Context {
        scope: Rc::clone(&context.scope),
        in_cfg_test: context.in_cfg_test,
        placement: Placement::Body,
        container: None,
        function: Some(Rc::new(Function {
            caller: caller(node, src, context),
            bindings: bindings::collect(node, src),
        })),
    })
}

fn visit_call<'tree>(
    node: Node<'tree>,
    src: &[u8],
    context: Rc<Context>,
    blocks: &mut Blocks<'tree>,
    out: &mut Extract,
) -> Rc<Context> {
    if let Some(reference) = call(node, src, &context, blocks) {
        out.refs.push(reference);
    }
    clear_direct(context)
}

fn visit_macro<'tree>(
    node: Node<'tree>,
    src: &[u8],
    context: Rc<Context>,
    blocks: &mut Blocks<'tree>,
    out: &mut Extract,
) -> Rc<Context> {
    if let Some(reference) = macro_call(node, src, &context) {
        out.refs.push(reference);
    }
    for shape in call_shapes(node, src) {
        let reference = macro_arg(shape, src, &context, blocks);
        out.refs.push(reference);
    }
    clear_direct(context)
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
fn call<'tree>(
    node: Node<'tree>,
    src: &[u8],
    context: &Context,
    blocks: &mut Blocks<'tree>,
) -> Option<Reference> {
    let function = node.child_by_field_name("function")?;
    let callee = match function.kind() {
        "identifier" | "scoped_identifier" => path_callee(function, function, src),
        "field_expression" => method_callee(function, None, src, blocks),
        "generic_function" => {
            let inner = function.child_by_field_name("function")?;
            match inner.kind() {
                "identifier" | "scoped_identifier" => path_callee(function, inner, src),
                "field_expression" => method_callee(
                    inner,
                    function.child_by_field_name("type_arguments"),
                    src,
                    blocks,
                ),
                _ => None,
            }
        }
        _ => None,
    }?;
    let shadowed = callee.kind == RefKind::Call && shadows(&callee, context);
    Some(reference(callee, shadowed, context))
}

/// A `MacroArg` reference. A method name's receiver is kept as written,
/// and one identifier takes its hint from the binding walk at that token;
/// a path of one segment is shadowed as a call's would be.
fn macro_arg<'tree>(
    shape: CallShape<'tree>,
    src: &[u8],
    context: &Context,
    blocks: &mut Blocks<'tree>,
) -> Reference {
    let (receiver, receiver_type) = match shape.receiver {
        None => (None, None),
        Some(Receiver::SelfValue) => (Some("self".to_owned()), None),
        Some(Receiver::Name(name)) => {
            (Some(compact(name, src)), receivers::hint(name, src, blocks))
        }
        Some(Receiver::Other(text)) => (Some(text), None),
    };
    let callee = Callee {
        kind: RefKind::MacroArg,
        text: shape.text,
        last_segment: shape.last_segment,
        at: shape.at,
        receiver,
        receiver_type,
        single_segment: shape.single_segment,
    };
    let shadowed = shadows(&callee, context);
    reference(callee, shadowed, context)
}

/// True for a single-segment callee whose name the enclosing function binds.
fn shadows(callee: &Callee<'_>, context: &Context) -> bool {
    callee.single_segment
        && context
            .function
            .as_ref()
            .is_some_and(|function| function.bindings.contains(&callee.last_segment))
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
    blocks: &mut Blocks<'tree>,
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
        receiver_type: value.and_then(|value| receivers::receiver_hint(value, src, blocks)),
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
        scope: (*context.scope).clone(),
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
fn caller(function: Node<'_>, src: &[u8], context: &Context) -> Caller {
    let name = function
        .child_by_field_name("name")
        .map(|name| compact(name, src))
        .unwrap_or_default();
    let owner = match &context.container {
        Some(Container::Impl { owner, .. }) | Some(Container::Trait(owner)) => Some(owner.clone()),
        None => None,
    };
    Caller {
        name,
        owner,
        start_line: start_line(function),
    }
}

/// A method record for a `fn` directly inside an `impl` at module scope.
fn method(function: Node<'_>, src: &[u8], context: &Context) -> Option<Method> {
    let Some(Container::Impl {
        owner,
        owner_path,
        trait_name,
    }) = &context.container
    else {
        return None;
    };
    let mut cursor = function.walk();
    let visibility = function
        .children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .map_or(Visibility::Private, |modifier| visibility(modifier, src));
    let detail = detail::detail(function, src);
    Some(Method {
        owner: owner.clone(),
        owner_path: owner_path.clone(),
        trait_name: trait_name.clone(),
        name: compact(function.child_by_field_name("name")?, src),
        visibility,
        scope: (*context.scope).clone(),
        start_line: start_line(function),
        end_line: detail.end_line,
        signature: detail.signature,
        doc_summary: detail.doc_summary,
        is_test: attrs::is_test(function, src, context.in_cfg_test),
    })
}

/// A self type's last path segment without generics: `a::Ledger<T>` and
/// `&Ledger` give `Ledger`. Other type shapes come back as written.
pub(crate) fn owner_name(self_type: Node<'_>, src: &[u8]) -> String {
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
