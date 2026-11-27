//! Top-level Go item, import, method, and embed extraction.

use std::collections::HashMap;

use phantom_fractal_lang::{
    Caller, Extract, Import, Item, ItemKind, Method, RefKind, Reference, Visibility,
};
use tree_sitter::Node;

use super::detail::{
    bare_type_name, doc_summary, end_line, go_visibility, is_go_test_fn, node_text, signature,
    start_col, start_line, strip_whitespace, type_last_segment,
};

/// Parsed `//go:linkname <local> [<target>]` directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinknameDirective {
    pub local_name: String,
    pub target: Option<String>,
    pub line: u32,
}

/// Body work queued while scanning top-level package declarations.
pub(crate) struct BodyTask<'t> {
    pub(crate) node: Node<'t>,
    pub(crate) caller: Option<Caller>,
    pub(crate) locals: Vec<(String, Option<String>)>,
}

/// Per-file lookup context built from top-level package declarations.
#[derive(Default)]
pub(crate) struct FileDecls<'t> {
    pub(crate) imports_by_qual: HashMap<String, String>,
    pub(crate) struct_fields: HashMap<(String, String), String>,
    pub(crate) pkg_var_types: HashMap<String, String>,
    pub(crate) bodies: Vec<BodyTask<'t>>,
}

/// Collects `//go:linkname` compiler directives from Go source text.
pub fn parse_linknames(source: &str) -> Vec<LinknameDirective> {
    let mut out = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let Some(dir) = parse_linkname_line(line.trim(), idx + 1) else {
            continue;
        };
        out.push(dir);
    }
    out
}

fn parse_linkname_line(trimmed: &str, line_num: usize) -> Option<LinknameDirective> {
    let rest = trimmed.strip_prefix("//go:linkname")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut parts = rest.split_whitespace();
    let local = parts.next()?;
    Some(LinknameDirective {
        local_name: local.to_owned(),
        target: parts.next().map(str::to_owned),
        line: u32::try_from(line_num).unwrap_or(u32::MAX),
    })
}

/// Derives Go default package qualifier from an import path (skipping `/vN`).
pub fn default_import_qualifier(path: &str) -> &str {
    let mut parts = path.rsplit('/');
    let last = parts.next().unwrap_or(path);
    if is_major_version_segment(last)
        && let Some(prev) = parts.next()
    {
        return prev;
    }
    last
}

fn is_major_version_segment(seg: &str) -> bool {
    let Some(digits) = seg.strip_prefix('v') else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits.parse::<u32>().is_ok_and(|n| n >= 2)
}

pub(crate) fn collect_top_level<'t>(
    root: Node<'t>,
    src: &[u8],
    out: &mut Extract,
) -> FileDecls<'t> {
    let mut decls = FileDecls::default();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        dispatch_top_level(child, src, out, &mut decls);
    }
    decls
}

fn dispatch_top_level<'t>(
    child: Node<'t>,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'t>,
) {
    match child.kind() {
        "import_declaration" => extract_import_decl(child, src, out, decls),
        "function_declaration" => extract_function(child, src, out, decls),
        "method_declaration" => extract_method(child, src, out, decls),
        _ => dispatch_type_or_val(child, src, out, decls),
    }
}

fn dispatch_type_or_val<'t>(
    child: Node<'t>,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'t>,
) {
    match child.kind() {
        "type_declaration" => extract_type_decl(child, src, out, decls),
        "var_declaration" => extract_val_decl(child, ItemKind::Var, src, out, decls),
        "const_declaration" => extract_val_decl(child, ItemKind::Const, src, out, decls),
        _ => {}
    }
}

fn extract_import_decl(decl: Node<'_>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'_>) {
    for spec in collect_specs(decl, "import_spec", "import_spec_list") {
        record_import_spec(spec, src, out, decls);
    }
}

fn collect_specs<'t>(decl: Node<'t>, spec_kind: &str, list_kind: &str) -> Vec<Node<'t>> {
    let mut specs = Vec::new();
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        if child.kind() == spec_kind {
            specs.push(child);
        } else if child.kind() == list_kind {
            let mut inner = child.walk();
            for spec in child.children(&mut inner) {
                if spec.kind() == spec_kind {
                    specs.push(spec);
                }
            }
        }
    }
    specs
}

fn record_import_spec(spec: Node<'_>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'_>) {
    let Some(path_node) = spec.child_by_field_name("path") else {
        return;
    };
    let path = node_text(path_node, src)
        .trim()
        .trim_matches(['"', '`'])
        .to_owned();
    let (alias, glob) = parse_import_alias(spec, src);
    if !glob && alias.as_deref() != Some("_") {
        let qual = alias
            .clone()
            .unwrap_or_else(|| default_import_qualifier(&path).to_owned());
        decls.imports_by_qual.insert(qual, path.clone());
    }
    out.imports.push(Import {
        path,
        alias,
        glob,
        visibility: Visibility::Private,
        start_line: start_line(spec),
    });
    out.import_scopes.push(String::new());
}

fn parse_import_alias(spec: Node<'_>, src: &[u8]) -> (Option<String>, bool) {
    match spec.child_by_field_name("name") {
        Some(node) if node.kind() == "dot" => (None, true),
        Some(node) => (Some(node_text(node, src)), false),
        None => (None, false),
    }
}

fn extract_function<'t>(node: Node<'t>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'t>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, src);
    let sline = start_line(node);
    if name != "_" {
        push_fn_item(node, &name, sline, src, out);
    }
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let locals = collect_fn_locals(node, None, src);
    let caller = (name != "_").then_some(Caller {
        name,
        owner: None,
        start_line: sline,
    });
    decls.bodies.push(BodyTask {
        node: body,
        caller,
        locals,
    });
}

fn push_fn_item(node: Node<'_>, name: &str, sline: u32, src: &[u8], out: &mut Extract) {
    out.items.push(Item {
        kind: ItemKind::Function,
        name: name.to_owned(),
        visibility: go_visibility(name),
        start_line: sline,
        end_line: end_line(node),
        signature: signature(node, src),
        doc_summary: doc_summary(node, src),
        is_test: is_go_test_fn(name),
    });
    out.item_scopes.push(String::new());
    out.item_owners.push(None);
}

fn collect_fn_locals(
    node: Node<'_>,
    recv: Option<(String, String)>,
    src: &[u8],
) -> Vec<(String, Option<String>)> {
    let mut locals = Vec::new();
    if let Some((rname, owner)) = recv {
        locals.push((rname, Some(format!("type:{owner}"))));
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        collect_param_bindings(params, src, &mut locals);
    }
    locals
}

fn extract_method<'t>(node: Node<'t>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'t>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(recv_list) = node.child_by_field_name("receiver") else {
        return;
    };
    let Some((recv_name, owner_path, owner)) = extract_receiver(recv_list, src) else {
        return;
    };
    let name = node_text(name_node, src);
    let sline = start_line(node);
    out.methods.push(Method {
        owner: owner.clone(),
        owner_path,
        trait_name: None,
        name: name.clone(),
        visibility: go_visibility(&name),
        scope: String::new(),
        start_line: sline,
        end_line: end_line(node),
        signature: signature(node, src),
        doc_summary: doc_summary(node, src),
        is_test: false,
    });
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let locals = collect_fn_locals(node, recv_name.map(|r| (r, owner.clone())), src);
    decls.bodies.push(BodyTask {
        node: body,
        caller: Some(Caller {
            name,
            owner: Some(owner),
            start_line: sline,
        }),
        locals,
    });
}

fn extract_receiver(recv_list: Node<'_>, src: &[u8]) -> Option<(Option<String>, String, String)> {
    let mut cursor = recv_list.walk();
    let param = recv_list
        .children(&mut cursor)
        .find(|c| c.kind() == "parameter_declaration")?;
    let ty_node = param.child_by_field_name("type")?;
    let owner_path = strip_whitespace(&node_text(ty_node, src));
    let owner = type_last_segment(&bare_type_name(&owner_path));
    if owner.is_empty() {
        return None;
    }
    let recv_name = param.child_by_field_name("name").map(|n| node_text(n, src));
    Some((recv_name, owner_path, owner))
}

/// Collects parameter names and `type:<T>` hints from a Go `parameter_list`.
pub(crate) fn collect_param_bindings(
    param_list: Node<'_>,
    src: &[u8],
    out: &mut Vec<(String, Option<String>)>,
) {
    let mut cursor = param_list.walk();
    for child in param_list.children(&mut cursor) {
        if matches!(
            child.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            push_param_decl_bindings(child, src, out);
        }
    }
}

fn push_param_decl_bindings(decl: Node<'_>, src: &[u8], out: &mut Vec<(String, Option<String>)>) {
    let type_hint = decl
        .child_by_field_name("type")
        .map(|t| bare_type_name(&node_text(t, src)))
        .filter(|t| !t.is_empty())
        .map(|t| format!("type:{t}"));
    let mut names_cursor = decl.walk();
    for n in decl.children_by_field_name("name", &mut names_cursor) {
        let ident = node_text(n, src);
        if ident != "_" {
            out.push((ident, type_hint.clone()));
        }
    }
}

fn extract_type_decl(decl: Node<'_>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'_>) {
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "type_alias" => push_type_item(child, ItemKind::TypeAlias, src, out),
            "type_spec" => extract_type_spec(child, src, out, decls),
            _ => {}
        }
    }
}

fn push_type_item(node: Node<'_>, kind: ItemKind, src: &[u8], out: &mut Extract) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = node_text(name_node, src);
    if name == "_" {
        return;
    }
    out.items.push(Item {
        kind,
        name: name.clone(),
        visibility: go_visibility(&name),
        start_line: start_line(node),
        end_line: end_line(node),
        signature: signature(node, src),
        doc_summary: doc_summary(node, src),
        is_test: false,
    });
    out.item_scopes.push(String::new());
    out.item_owners.push(None);
}

fn extract_type_spec(spec: Node<'_>, src: &[u8], out: &mut Extract, decls: &mut FileDecls<'_>) {
    let Some(name_node) = spec.child_by_field_name("name") else {
        return;
    };
    let type_name = node_text(name_node, src);
    let Some(ty_node) = spec.child_by_field_name("type") else {
        return;
    };
    match ty_node.kind() {
        "struct_type" => {
            push_type_item(spec, ItemKind::Struct, src, out);
            extract_struct_fields(&type_name, ty_node, src, out, decls);
        }
        "interface_type" => {
            push_type_item(spec, ItemKind::Interface, src, out);
            extract_interface_members(&type_name, ty_node, src, out);
        }
        _ => push_type_item(spec, ItemKind::Struct, src, out),
    }
}

fn extract_struct_fields(
    struct_name: &str,
    struct_node: Node<'_>,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'_>,
) {
    let mut cur = struct_node.walk();
    let Some(list) = struct_node
        .children(&mut cur)
        .find(|c| c.kind() == "field_declaration_list")
    else {
        return;
    };
    let mut fields = list.walk();
    for field in list.children(&mut fields) {
        if field.kind() == "field_declaration" {
            record_struct_field(struct_name, field, src, out, decls);
        }
    }
}

fn record_struct_field(
    struct_name: &str,
    field: Node<'_>,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'_>,
) {
    let Some(ty) = field.child_by_field_name("type") else {
        return;
    };
    let type_text = bare_type_name(&node_text(ty, src));
    let mut names_cur = field.walk();
    let mut names = field
        .children_by_field_name("name", &mut names_cur)
        .peekable();
    if names.peek().is_none() {
        push_embed_ref(struct_name, &type_text, ty, out);
        return;
    }
    for name_node in names {
        let fname = node_text(name_node, src);
        decls
            .struct_fields
            .insert((struct_name.to_owned(), fname), type_text.clone());
    }
}

fn extract_interface_members(
    iface_name: &str,
    iface_node: Node<'_>,
    src: &[u8],
    out: &mut Extract,
) {
    let mut cursor = iface_node.walk();
    for child in iface_node.children(&mut cursor) {
        if child.kind() == "method_elem" {
            push_interface_method(iface_name, child, src, out);
        } else if child.kind() == "type_elem" {
            let text = bare_type_name(&node_text(child, src));
            if is_simple_type_ref(&text) {
                push_embed_ref(iface_name, &text, child, out);
            }
        }
    }
}

fn push_interface_method(iface_name: &str, child: Node<'_>, src: &[u8], out: &mut Extract) {
    let Some(name_node) = child.child_by_field_name("name") else {
        return;
    };
    let mname = node_text(name_node, src);
    out.methods.push(Method {
        owner: iface_name.to_owned(),
        owner_path: iface_name.to_owned(),
        trait_name: None,
        name: mname.clone(),
        visibility: go_visibility(&mname),
        scope: String::new(),
        start_line: start_line(child),
        end_line: end_line(child),
        signature: signature(child, src),
        doc_summary: doc_summary(child, src),
        is_test: false,
    });
}

fn is_simple_type_ref(text: &str) -> bool {
    !text.is_empty() && !text.contains('|') && !text.starts_with('~')
}

fn push_embed_ref(owner: &str, type_text: &str, node: Node<'_>, out: &mut Extract) {
    if type_text.is_empty() {
        return;
    }
    out.refs.push(Reference {
        kind: RefKind::Embed,
        text: type_text.to_owned(),
        last_segment: type_last_segment(type_text),
        line: start_line(node),
        col: start_col(node),
        scope: String::new(),
        from: None,
        receiver: Some(owner.to_owned()),
        receiver_type: None,
        shadowed: false,
    });
}

fn extract_val_decl<'t>(
    decl: Node<'t>,
    kind: ItemKind,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'t>,
) {
    let (spec_kind, list_kind) = if kind == ItemKind::Var {
        ("var_spec", "var_spec_list")
    } else {
        ("const_spec", "const_spec_list")
    };
    for spec in collect_specs(decl, spec_kind, list_kind) {
        record_val_spec(spec, kind, src, out, decls);
    }
}

fn record_val_spec<'t>(
    spec: Node<'t>,
    kind: ItemKind,
    src: &[u8],
    out: &mut Extract,
    decls: &mut FileDecls<'t>,
) {
    let explicit_hint = spec
        .child_by_field_name("type")
        .map(|t| format!("type:{}", bare_type_name(&node_text(t, src))));
    let mut caller = None;
    let mut names_cur = spec.walk();
    for n in spec.children_by_field_name("name", &mut names_cur) {
        let name = node_text(n, src);
        if name != "_" {
            if let Some(hint) = explicit_hint.as_deref() {
                decls.pkg_var_types.insert(name.clone(), hint.to_owned());
            }
            push_val_item(spec, kind, &name, src, out);
            caller.get_or_insert(Caller {
                name,
                owner: None,
                start_line: start_line(spec),
            });
        }
    }
    if let Some(val) = spec.child_by_field_name("value") {
        decls.bodies.push(BodyTask {
            node: val,
            caller,
            locals: Vec::new(),
        });
    }
}

fn push_val_item(spec: Node<'_>, kind: ItemKind, name: &str, src: &[u8], out: &mut Extract) {
    out.items.push(Item {
        kind,
        name: name.to_owned(),
        visibility: go_visibility(name),
        start_line: start_line(spec),
        end_line: end_line(spec),
        signature: signature(spec, src),
        doc_summary: doc_summary(spec, src),
        is_test: false,
    });
    out.item_scopes.push(String::new());
    out.item_owners.push(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    #[test]
    fn checks_import_qualifiers_linknames_and_interface_embeds() {
        assert_eq!(default_import_qualifier("example.com/mod/v2"), "mod");
        assert_eq!(default_import_qualifier("example.com/mod/v1"), "v1");
        assert_eq!(default_import_qualifier("example.com/mod/vx"), "vx");
        assert_eq!(default_import_qualifier("example.com/mod/v"), "v");
        assert_eq!(default_import_qualifier("fmt"), "fmt");

        let ln = parse_linknames("\n//go:linkname foo pkg.Bar\n");
        assert_eq!(ln.len(), 1);
        assert_eq!(ln[0].line, 2);

        let src = "package p\nimport (\n\t. \"dot/pkg\"\n\t_ \"unsafe\"\n\t\"fmt\"\n)\ntype I interface {\n\tfmt.Stringer\n\t~int | string\n\tM()\n}\nfunc f() { _ = unsafe.Pointer(nil); _ = dot.X() }\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_go::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(src, None).unwrap();
        let mut ext = Extract::default();
        let decls = collect_top_level(tree.root_node(), src.as_bytes(), &mut ext);
        assert!(decls.imports_by_qual.contains_key("fmt"));
        assert!(!decls.imports_by_qual.contains_key("unsafe"));
        assert!(!decls.imports_by_qual.contains_key("pkg"));
        let embeds: Vec<&str> = ext
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::Embed)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(embeds, vec!["fmt.Stringer"]);
        assert!(is_simple_type_ref("Reader"));
        assert!(!is_simple_type_ref(""));
        assert!(!is_simple_type_ref("int|string"));
        assert!(!is_simple_type_ref("~int"));
    }
}
