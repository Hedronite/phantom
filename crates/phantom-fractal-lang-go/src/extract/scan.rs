//! Body and initializer scanner for Go call references, method hints, and value refs.

use phantom_fractal_lang::{Caller, Extract, RefKind, Reference};
use tree_sitter::Node;

use super::detail::{
    bare_type_name, node_text, start_col, start_line, strip_whitespace, type_hint_from_node,
};
use super::items::{FileDecls, collect_param_bindings};

struct ScopeEnv {
    bindings: Vec<(String, Option<String>)>,
}

impl ScopeEnv {
    fn from_initial(initial: Vec<(String, Option<String>)>) -> Self {
        Self { bindings: initial }
    }

    fn checkpoint(&self) -> usize {
        self.bindings.len()
    }

    fn rollback(&mut self, mark: usize) {
        self.bindings.truncate(mark);
    }

    fn push(&mut self, name: String, hint: Option<String>) {
        if name != "_" {
            self.bindings.push((name, hint));
        }
    }

    fn is_bound(&self, name: &str) -> bool {
        self.bindings.iter().any(|(n, _)| n == name)
    }

    fn hint(&self, name: &str) -> Option<&str> {
        self.bindings
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, h)| h.as_deref())
    }
}

struct ScanCtx<'a> {
    src: &'a [u8],
    decls: &'a FileDecls<'a>,
    caller: Option<Caller>,
    out: &'a mut Extract,
}

pub(crate) fn scan_bodies<'t>(decls: &FileDecls<'t>, src: &[u8], out: &mut Extract) {
    for task in &decls.bodies {
        let mut env = ScopeEnv::from_initial(task.locals.clone());
        let mut ctx = ScanCtx {
            src,
            decls,
            caller: task.caller.clone(),
            out,
        };
        scan_node(task.node, &mut env, &mut ctx);
    }
    out.refs.sort_by_key(|r| (r.line, r.col));
}

fn is_block_scope_kind(kind: &str) -> bool {
    matches!(
        kind,
        "block"
            | "if_statement"
            | "for_statement"
            | "expression_switch_statement"
            | "type_switch_statement"
            | "select_statement"
            | "expression_case"
            | "type_case"
            | "default_case"
            | "communication_case"
    )
}

fn scan_node(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let kind = node.kind();
    if is_block_scope_kind(kind) {
        let mark = env.checkpoint();
        scan_children(node, env, ctx);
        env.rollback(mark);
        return;
    }
    if scan_stmt_node(node, kind, env, ctx) {
        return;
    }
    scan_expr_node(node, kind, env, ctx);
}

fn scan_stmt_node(node: Node<'_>, kind: &str, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) -> bool {
    match kind {
        "func_literal" => scan_func_literal(node, env, ctx),
        "short_var_declaration" => scan_short_var_decl(node, env, ctx),
        "assignment_statement" => scan_assignment(node, env, ctx),
        _ => return scan_var_or_range_stmt(node, kind, env, ctx),
    }
    true
}

fn scan_var_or_range_stmt(
    node: Node<'_>,
    kind: &str,
    env: &mut ScopeEnv,
    ctx: &mut ScanCtx<'_>,
) -> bool {
    match kind {
        "var_spec" | "const_spec" => scan_local_spec(node, env, ctx),
        "range_clause" => scan_range_clause(node, env, ctx),
        _ => return false,
    }
    true
}

fn scan_expr_node(node: Node<'_>, kind: &str, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    match kind {
        "call_expression" => scan_call_expr(node, env, ctx),
        "selector_expression" => scan_value_selector(node, env, ctx),
        "identifier" => record_ident_value_ref(node, env, ctx),
        "binary_expression" if is_plus_chain(node, ctx.src) => scan_plus_chain(node, env, ctx),
        _ => scan_wrapped_or_children(node, kind, env, ctx),
    }
}

fn is_plus_chain(node: Node<'_>, src: &[u8]) -> bool {
    node.child_by_field_name("operator")
        .is_some_and(|o| node_text(o, src) == "+")
}

fn is_plus_binary(node: Node<'_>, src: &[u8]) -> bool {
    node.kind() == "binary_expression" && is_plus_chain(node, src)
}

fn scan_plus_chain(mut node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    loop {
        if !is_plus_binary(node, ctx.src) {
            scan_node(node, env, ctx);
            return;
        }
        if let Some(right) = node.child_by_field_name("right") {
            scan_node(right, env, ctx);
        }
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        if !is_plus_binary(left, ctx.src) {
            scan_node(left, env, ctx);
            return;
        }
        node = left;
    }
}

fn scan_wrapped_or_children(node: Node<'_>, kind: &str, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if kind == "keyed_element" {
        scan_field_only(node, "value", env, ctx);
    } else if kind == "parenthesized_expression" {
        scan_node(peel_parentheses(node), env, ctx);
    } else {
        scan_children(node, env, ctx);
    }
}

fn peel_parentheses(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(inner) = node.named_child(0) else {
            break;
        };
        node = inner;
    }
    node
}

fn lone_block_in_statement_list(stmt_list: Node<'_>) -> Option<Node<'_>> {
    let mut list_walk = stmt_list.walk();
    let stmts: Vec<Node<'_>> = stmt_list.named_children(&mut list_walk).collect();
    if stmts.len() != 1 || stmts[0].kind() != "block" {
        return None;
    }
    Some(stmts[0])
}

fn next_single_child_block(cur: Node<'_>) -> Option<Node<'_>> {
    if cur.kind() != "block" {
        return None;
    }
    let mut inner_walk = cur.walk();
    let kids: Vec<Node<'_>> = cur.named_children(&mut inner_walk).collect();
    if kids.len() != 1 || kids[0].kind() != "statement_list" {
        return None;
    }
    lone_block_in_statement_list(kids[0])
}

fn peel_single_child_blocks(mut cur: Node<'_>) -> Node<'_> {
    while let Some(next) = next_single_child_block(cur) {
        cur = next;
    }
    cur
}

fn scan_children(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scan_node(peel_single_child_blocks(child), env, ctx);
    }
}

fn scan_field_only(node: Node<'_>, field: &str, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if let Some(child) = node.child_by_field_name(field) {
        scan_node(child, env, ctx);
    }
}

fn scan_func_literal(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let mark = env.checkpoint();
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut pairs = Vec::new();
        collect_param_bindings(params, ctx.src, &mut pairs);
        for (name, hint) in pairs {
            env.push(name, hint);
        }
    }
    scan_field_only(node, "body", env, ctx);
    env.rollback(mark);
}

fn scan_assignment(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    scan_field_only(node, "right", env, ctx);
    if let Some(left) = node.child_by_field_name("left") {
        let mut cursor = left.walk();
        for child in left.children(&mut cursor) {
            if child.kind() == "index_expression" {
                scan_field_only(child, "index", env, ctx);
            }
        }
    }
}

fn scan_short_var_decl(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    scan_field_only(node, "right", env, ctx);
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    let lhs_names = collect_ident_list(left, ctx.src);
    let rhs_exprs = node
        .child_by_field_name("right")
        .map(collect_expr_list)
        .unwrap_or_default();
    bind_exprs(&lhs_names, &rhs_exprs, env, ctx);
}

fn scan_local_spec(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    scan_field_only(node, "value", env, ctx);
    let explicit = node
        .child_by_field_name("type")
        .and_then(|t| type_hint_from_node(t, ctx.src));
    let rhs_exprs = node
        .child_by_field_name("value")
        .map(collect_expr_list)
        .unwrap_or_default();
    let mut cur = node.walk();
    let names: Vec<String> = node
        .children_by_field_name("name", &mut cur)
        .map(|n| node_text(n, ctx.src))
        .collect();
    if let Some(hint) = explicit {
        for name in names {
            env.push(name, Some(hint.clone()));
        }
    } else {
        bind_exprs(&names, &rhs_exprs, env, ctx);
    }
}

fn scan_range_clause(node: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    scan_field_only(node, "right", env, ctx);
    let elem_hint = node
        .child_by_field_name("right")
        .and_then(|r| infer_expr_hint(r, false, env, ctx))
        .and_then(|h| h.strip_prefix("elem:").map(|t| format!("type:{t}")));
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    for (idx, name) in collect_ident_list(left, ctx.src).into_iter().enumerate() {
        let hint = (idx == 1).then(|| elem_hint.clone()).flatten();
        env.push(name, hint);
    }
}

fn bind_exprs(lhs_names: &[String], rhs_exprs: &[Node<'_>], env: &mut ScopeEnv, ctx: &ScanCtx<'_>) {
    let multi_from_single_call = lhs_names.len() > 1 && rhs_exprs.len() == 1;
    for (idx, name) in lhs_names.iter().enumerate() {
        let hint = if multi_from_single_call {
            infer_expr_hint(rhs_exprs[0], true, env, ctx)
        } else {
            rhs_exprs
                .get(idx)
                .and_then(|e| infer_expr_hint(*e, false, env, ctx))
        };
        env.push(name.clone(), hint);
    }
}

fn collect_ident_list(list: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut cur = list.walk();
    list.children(&mut cur)
        .filter(|c| c.kind() == "identifier")
        .map(|c| node_text(c, src))
        .collect()
}

fn collect_expr_list(node: Node<'_>) -> Vec<Node<'_>> {
    if node.kind() != "expression_list" {
        return vec![node];
    }
    let mut cur = node.walk();
    node.children(&mut cur).filter(|c| c.is_named()).collect()
}

fn scan_call_expr(call: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if let Some(callee) = call.child_by_field_name("function") {
        scan_call_callee(callee, env, ctx);
    }
    scan_field_only(call, "arguments", env, ctx);
}

fn scan_call_callee(callee: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    match callee.kind() {
        "identifier" => record_ident_call(callee, env, ctx),
        "selector_expression" => scan_call_selector(callee, env, ctx),
        _ => scan_other_callee(callee, env, ctx),
    }
}

fn scan_other_callee(callee: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if callee.kind() == "func_literal" {
        scan_func_literal(callee, env, ctx);
    } else if callee.kind() == "index_expression" {
        scan_indexed_callee(callee, env, ctx);
    }
}

fn record_ident_call(callee: Node<'_>, env: &ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let name = node_text(callee, ctx.src);
    ctx.out.refs.push(Reference {
        kind: RefKind::Call,
        text: name.clone(),
        last_segment: name.clone(),
        line: start_line(callee),
        col: start_col(callee),
        scope: String::new(),
        from: ctx.caller.clone(),
        receiver: None,
        receiver_type: None,
        shadowed: env.is_bound(&name),
    });
}

fn scan_indexed_callee(callee: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if let Some(op) = callee.child_by_field_name("operand")
        && let Some((text, last)) = static_callable_name(op, env, ctx)
    {
        ctx.out.refs.push(Reference {
            kind: RefKind::Call,
            text,
            last_segment: last,
            line: start_line(callee),
            col: start_col(callee),
            scope: String::new(),
            from: ctx.caller.clone(),
            receiver: None,
            receiver_type: Some("type_inst".to_owned()),
            shadowed: false,
        });
        return;
    }
    scan_field_only(callee, "index", env, ctx);
}

fn static_callable_name(
    node: Node<'_>,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
) -> Option<(String, String)> {
    if node.kind() == "identifier" {
        let name = node_text(node, ctx.src);
        return (!env.is_bound(&name)).then(|| (name.clone(), name));
    }
    if node.kind() == "selector_expression" {
        return imported_selector_name(node, env, ctx);
    }
    None
}

fn imported_selector_name(
    sel: Node<'_>,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
) -> Option<(String, String)> {
    let pkg = sel.child_by_field_name("operand")?;
    let field = sel.child_by_field_name("field")?;
    if pkg.kind() != "identifier" {
        return None;
    }
    let pkg_name = node_text(pkg, ctx.src);
    if env.is_bound(&pkg_name) || !ctx.decls.imports_by_qual.contains_key(&pkg_name) {
        return None;
    }
    let fn_name = node_text(field, ctx.src);
    Some((format!("{pkg_name}.{fn_name}"), fn_name))
}

fn scan_call_selector(sel: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    if let Some((text, last)) = imported_selector_name(sel, env, ctx) {
        ctx.out.refs.push(Reference {
            kind: RefKind::Call,
            text,
            last_segment: last,
            line: start_line(sel),
            col: start_col(sel),
            scope: String::new(),
            from: ctx.caller.clone(),
            receiver: None,
            receiver_type: None,
            shadowed: false,
        });
        return;
    }
    let (Some(operand), Some(field)) = (
        sel.child_by_field_name("operand"),
        sel.child_by_field_name("field"),
    ) else {
        return;
    };
    let method = node_text(field, ctx.src);
    let recv_hint = infer_expr_hint(operand, false, env, ctx);
    let recv_text = strip_whitespace(&node_text(operand, ctx.src));
    ctx.out.refs.push(Reference {
        kind: RefKind::MethodCall,
        text: method.clone(),
        last_segment: method,
        line: start_line(sel),
        col: start_col(sel),
        scope: String::new(),
        from: ctx.caller.clone(),
        receiver: Some(recv_text),
        receiver_type: recv_hint,
        shadowed: false,
    });
    if !matches!(operand.kind(), "identifier" | "selector_expression") {
        scan_node(operand, env, ctx);
    }
}

fn scan_value_selector(sel: Node<'_>, env: &mut ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let (Some(operand), Some(field)) = (
        sel.child_by_field_name("operand"),
        sel.child_by_field_name("field"),
    ) else {
        return;
    };
    let field_name = node_text(field, ctx.src);
    if !matches!(operand.kind(), "identifier" | "selector_expression") {
        scan_node(operand, env, ctx);
    }
    if is_known_struct_field(operand, &field_name, env, ctx) {
        return;
    }
    ctx.out.refs.push(Reference {
        kind: RefKind::ValueRef,
        text: strip_whitespace(&node_text(sel, ctx.src)),
        last_segment: field_name,
        line: start_line(sel),
        col: start_col(sel),
        scope: String::new(),
        from: ctx.caller.clone(),
        receiver: None,
        receiver_type: None,
        shadowed: false,
    });
}

fn is_known_struct_field(
    operand: Node<'_>,
    field_name: &str,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
) -> bool {
    infer_expr_hint(operand, false, env, ctx)
        .and_then(|h| h.strip_prefix("type:").map(str::to_owned))
        .is_some_and(|ty| {
            ctx.decls
                .struct_fields
                .contains_key(&(ty, field_name.to_owned()))
        })
}

fn record_ident_value_ref(ident: Node<'_>, env: &ScopeEnv, ctx: &mut ScanCtx<'_>) {
    let name = node_text(ident, ctx.src);
    if matches!(name.as_str(), "_" | "nil" | "true" | "false" | "iota") || env.is_bound(&name) {
        return;
    }
    ctx.out.refs.push(Reference {
        kind: RefKind::ValueRef,
        text: name.clone(),
        last_segment: name,
        line: start_line(ident),
        col: start_col(ident),
        scope: String::new(),
        from: ctx.caller.clone(),
        receiver: None,
        receiver_type: None,
        shadowed: false,
    });
}

const MAX_HINT_DEPTH: u8 = 8;

fn infer_expr_hint(
    expr: Node<'_>,
    multi_return: bool,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
) -> Option<String> {
    infer_expr_hint_d(expr, multi_return, env, ctx, 0)
}

fn infer_expr_hint_d(
    expr: Node<'_>,
    multi_return: bool,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    if depth >= MAX_HINT_DEPTH {
        return None;
    }
    infer_expr_hint_inner(expr, multi_return, env, ctx, depth)
}

fn infer_expr_hint_inner(
    expr: Node<'_>,
    multi_return: bool,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    match expr.kind() {
        "identifier" => infer_ident_hint(expr, env, ctx),
        "composite_literal" | "type_assertion_expression" => type_field_hint(expr, ctx.src),
        "call_expression" => infer_call_return_hint(expr, multi_return, env, ctx, depth),
        _ => infer_compound_hint(expr, multi_return, env, ctx, depth),
    }
}

fn infer_compound_hint(
    expr: Node<'_>,
    multi_return: bool,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    match expr.kind() {
        "unary_expression" => expr
            .child_by_field_name("operand")
            .and_then(|op| infer_expr_hint_d(op, multi_return, env, ctx, depth + 1)),
        "parenthesized_expression" => expr
            .named_child(0)
            .and_then(|inner| infer_expr_hint_d(inner, multi_return, env, ctx, depth + 1)),
        "index_expression" => infer_index_elem_hint(expr, env, ctx, depth),
        "selector_expression" => infer_selector_field_hint(expr, env, ctx, depth),
        _ => None,
    }
}

fn infer_index_elem_hint(
    expr: Node<'_>,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    let op = expr.child_by_field_name("operand")?;
    let hint = infer_expr_hint_d(op, false, env, ctx, depth + 1)?;
    hint.strip_prefix("elem:").map(|t| format!("type:{t}"))
}

fn infer_ident_hint(expr: Node<'_>, env: &ScopeEnv, ctx: &ScanCtx<'_>) -> Option<String> {
    let name = node_text(expr, ctx.src);
    env.hint(&name)
        .map(str::to_owned)
        .or_else(|| ctx.decls.pkg_var_types.get(&name).cloned())
}

fn type_field_hint(node: Node<'_>, src: &[u8]) -> Option<String> {
    node.child_by_field_name("type")
        .and_then(|t| type_hint_from_node(t, src))
}

fn infer_selector_field_hint(
    sel: Node<'_>,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    let op = sel.child_by_field_name("operand")?;
    let field = node_text(sel.child_by_field_name("field")?, ctx.src);
    let owner_hint = infer_expr_hint_d(op, false, env, ctx, depth + 1)?;
    if let Some(owner) = owner_hint.strip_prefix("type:")
        && let Some(found) = ctx
            .decls
            .struct_fields
            .get(&(owner.to_owned(), field.clone()))
    {
        return Some(found.clone());
    }
    Some(format!("field:{owner_hint}|{field}"))
}

fn infer_call_return_hint(
    call: Node<'_>,
    multi_return: bool,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    let func = call.child_by_field_name("function")?;
    if let Some(builtin) = infer_new_builtin_hint(call, func, env, ctx.src) {
        return Some(builtin);
    }
    let prefix = if multi_return { "ret?:" } else { "ret:" };
    infer_static_or_inst_hint(func, prefix, env, ctx)
        .or_else(|| infer_method_return_hint(func, env, ctx, depth))
}

fn infer_static_or_inst_hint(
    func: Node<'_>,
    prefix: &str,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
) -> Option<String> {
    if func.kind() == "index_expression" {
        let op = func.child_by_field_name("operand")?;
        return static_callable_name(op, env, ctx).map(|(n, _)| format!("inst_{prefix}{n}"));
    }
    static_callable_name(func, env, ctx).map(|(n, _)| format!("{prefix}{}", n))
}

fn infer_new_builtin_hint(
    call: Node<'_>,
    func: Node<'_>,
    env: &ScopeEnv,
    src: &[u8],
) -> Option<String> {
    if func.kind() != "identifier" || node_text(func, src) != "new" || env.is_bound("new") {
        return None;
    }
    let arg = call.child_by_field_name("arguments")?.named_child(0)?;
    let name = bare_type_name(&node_text(arg, src));
    (!name.is_empty()).then(|| format!("type:{name}"))
}

fn infer_method_return_hint(
    func: Node<'_>,
    env: &ScopeEnv,
    ctx: &ScanCtx<'_>,
    depth: u8,
) -> Option<String> {
    if func.kind() != "selector_expression" {
        return None;
    }
    let op = func.child_by_field_name("operand")?;
    let field = func.child_by_field_name("field")?;
    let recv_hint = infer_expr_hint_d(op, false, env, ctx, depth + 1)?;
    Some(format!("mret:{recv_hint}|{}", node_text(field, ctx.src)))
}

#[cfg(test)]
mod peel_tests {
    use super::*;
    use crate::GoLanguage;
    use phantom_fractal_lang::{Language, RefKind};
    use tree_sitter::{Node, Parser, Tree};

    fn parse_go(source: &str) -> Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("grammar");
        parser.parse(source, None).expect("parse")
    }

    fn first_binary(root: Node<'_>) -> Option<Node<'_>> {
        if root.kind() == "binary_expression" {
            return Some(root);
        }
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if let Some(found) = first_binary(child) {
                return Some(found);
            }
        }
        None
    }

    fn max_single_block_chain_depth(mut cur: Node<'_>) -> usize {
        let mut depth = 0usize;
        while let Some(next) = next_single_child_block(cur) {
            depth += 1;
            cur = next;
        }
        depth
    }

    fn func_body_decl(root: Node<'_>) -> Option<Node<'_>> {
        if root.kind() == "function_declaration" {
            return root.child_by_field_name("body");
        }
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if let Some(body) = func_body_decl(child) {
                return Some(body);
            }
        }
        None
    }

    fn deepest_peelable_block(root: Node<'_>) -> Option<(Node<'_>, usize)> {
        let mut best: Option<(Node<'_>, usize)> = None;
        fn visit<'a>(node: Node<'a>, best: &mut Option<(Node<'a>, usize)>) {
            if node.kind() == "block" {
                let depth = max_single_block_chain_depth(node);
                if depth > 0 && best.as_ref().is_none_or(|(_, d)| depth > *d) {
                    *best = Some((node, depth));
                }
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                visit(child, best);
            }
        }
        visit(root, &mut best);
        best
    }

    #[test]
    fn is_plus_chain_rejects_subtraction() {
        let src = "package p\nvar x = a - b\n";
        let tree = parse_go(src);
        let bin = first_binary(tree.root_node()).expect("binary");
        assert!(!is_plus_chain(bin, src.as_bytes()));
    }

    #[test]
    fn is_plus_binary_requires_plus_binary_shape() {
        let minus = "package p\nvar x = a - b\n";
        let tree = parse_go(minus);
        let bin = first_binary(tree.root_node()).expect("binary");
        assert!(!is_plus_binary(bin, minus.as_bytes()));
        let plus = "package p\nvar x = a + b\n";
        let tree2 = parse_go(plus);
        let bin2 = first_binary(tree2.root_node()).expect("binary");
        assert!(is_plus_binary(bin2, plus.as_bytes()));
    }

    #[test]
    fn plus_chain_scan_visits_only_on_plus_path() {
        let lang = GoLanguage::new();
        let src = "package p\nfunc F() int { return (((((((\"a\" + \"b\") + Side()) + 1))))); }\n";
        let ext = lang.extract(src).unwrap();
        assert!(
            ext.refs
                .iter()
                .any(|r| r.kind == RefKind::Call && r.text == "Side")
        );
        let minus = lang
            .extract("package p\nfunc G() { _ = a - callee() }\n")
            .unwrap();
        assert!(
            minus
                .refs
                .iter()
                .any(|r| r.kind == RefKind::Call && r.text == "callee")
        );
    }

    #[test]
    fn next_single_child_block_none_when_not_a_chain() {
        let tree = parse_go("package p\nfunc G() { return 1 }\n");
        let body = func_body_decl(tree.root_node()).expect("body");
        assert!(next_single_child_block(body).is_none());
        let tree2 = parse_go("package p\nfunc G() { A(); B() }\n");
        let body2 = func_body_decl(tree2.root_node()).expect("body");
        assert!(next_single_child_block(body2).is_none());
        let tree3 = parse_go("package p\nfunc G() int { x := 1; return x }\n");
        let body3 = func_body_decl(tree3.root_node()).expect("body");
        assert!(next_single_child_block(body3).is_none());
    }

    #[test]
    fn plus_chain_minus_on_left_spine_still_finds_calls() {
        let lang = GoLanguage::new();
        let ext = lang
            .extract("package p\nfunc F() int { return (f2() - f3()) + f1() + f4() }\n")
            .unwrap();
        for name in ["f1", "f2", "f3", "f4"] {
            assert!(
                ext.refs
                    .iter()
                    .any(|r| r.kind == RefKind::Call && r.text == name),
                "missing {name}"
            );
        }
    }

    #[test]
    fn plus_chain_spine_and_minus_rhs_are_collected() {
        let lang = GoLanguage::new();
        let ext = lang
            .extract("package p\nfunc F() { _ = A() + (B() - C()) }\n")
            .unwrap();
        for name in ["A", "B", "C"] {
            assert!(
                ext.refs
                    .iter()
                    .any(|r| r.kind == RefKind::Call && r.text == name),
                "missing call {name}"
            );
        }
        let mut src = String::from("package p\nfunc F() int { return f1()");
        for n in 2..=12 {
            src.push_str(&format!(" + f{n}()"));
        }
        src.push_str(" }\n");
        let ext = lang.extract(&src).unwrap();
        for n in 1..=12 {
            let want = format!("f{n}");
            assert!(
                ext.refs
                    .iter()
                    .any(|r| r.kind == RefKind::Call && r.text == want),
                "missing {want}"
            );
        }
    }

    #[test]
    fn peel_single_child_blocks_reaches_return_block() {
        let mut src = String::from("package p\nfunc G() int {\n");
        for _ in 0..24 {
            src.push('{');
        }
        src.push_str("return Tail()\n");
        for _ in 0..24 {
            src.push('}');
        }
        src.push_str("\n}\n");
        let tree = parse_go(&src);
        let (start, depth) = deepest_peelable_block(tree.root_node()).expect("peelable chain");
        assert!(depth >= 20, "expected deep single-block chain, got {depth}");
        let peeled = peel_single_child_blocks(start);
        assert!(
            max_single_block_chain_depth(peeled) < depth,
            "peel should shorten the chain"
        );
        let stmt_list = peeled
            .named_child(0)
            .filter(|n| n.kind() == "statement_list")
            .expect("statement_list");
        let mut walk = stmt_list.walk();
        assert!(
            stmt_list
                .named_children(&mut walk)
                .any(|c| c.kind() == "return_statement"),
            "peel should expose return in the inner statement list"
        );
    }

    #[test]
    fn single_block_chain_still_reaches_call() {
        let lang = GoLanguage::new();
        let mut src = String::from("package p\nfunc G() int {\n");
        for _ in 0..24 {
            src.push('{');
        }
        src.push_str("return Tail()\n");
        for _ in 0..24 {
            src.push('}');
        }
        src.push_str("\n}\nfunc Tail() int { return 1 }\n");
        let ext = lang.extract(&src).unwrap();
        assert!(
            ext.refs
                .iter()
                .any(|r| r.kind == RefKind::Call && r.text == "Tail")
        );
    }
}
