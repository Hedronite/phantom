//! Rust source extractor for Phantom.
//!
//! Parses with `tree-sitter-rust` and runs one query written for this crate,
//! `queries/extract.scm`. Only module-scope declarations are extracted: those
//! in the file body or in an inline `mod` body. Declarations inside function
//! bodies, `impl` blocks, and trait bodies are skipped.
//!
//! A second pass over the tree records call references, the methods of
//! module-scope `impl` blocks, and `mod` declarations. Nothing is resolved,
//! and macros are never expanded: a call-shaped name inside a macro's
//! tokens is recorded as written.
#![forbid(unsafe_code)]

mod attrs;
mod bindings;
mod detail;
mod imports;
mod macro_args;
mod placed;
mod receivers;
mod resolve;
mod scan;
mod std_methods;

use std::collections::HashMap;
use std::rc::Rc;

use phantom_fractal_lang::{Extract, ExtractError, Item, ItemKind, Language, Visibility};

/// Maximum nested inner `fn` depth indexed under an enclosing function or method.
pub const MAX_NESTED_FN_DEPTH: usize = 16;
pub use receivers::MAX_HINT_VISITS;
pub use resolve::{confidence, links, method_owners, resolve, resolve_graph};
pub use std_methods::STD_METHOD_NAMES;
use tree_sitter::{Node, Parser, Query, QueryCursor, QueryMatch, StreamingIterator};

const QUERY: &str = include_str!("../queries/extract.scm");

/// The Rust extractor. Build once and reuse; the query compiles in [`new`].
///
/// [`new`]: RustLanguage::new
pub struct RustLanguage {
    query: Query,
    captures: Captures,
}

/// Indices of the inner captures in [`QUERY`]. Every other capture names a
/// record kind.
struct Captures {
    visibility: u32,
    name: u32,
    argument: u32,
}

/// What one query match records.
enum Record {
    Item(ItemKind),
    Use,
}

/// The captured nodes of one query match.
#[derive(Default)]
struct Found<'tree> {
    record: Option<(Record, Node<'tree>)>,
    name: Option<Node<'tree>>,
    visibility: Option<Node<'tree>>,
    argument: Option<Node<'tree>>,
}

impl RustLanguage {
    /// Compiles the extract query against the bundled grammar.
    pub fn new() -> Result<Self, ExtractError> {
        let query = Query::new(&tree_sitter_rust::LANGUAGE.into(), QUERY)
            .map_err(|err| ExtractError::Grammar(err.to_string()))?;
        let index = |name: &str| {
            query
                .capture_index_for_name(name)
                .ok_or_else(|| ExtractError::Grammar(format!("query has no @{name} capture")))
        };
        let captures = Captures {
            visibility: index("visibility")?,
            name: index("name")?,
            argument: index("argument")?,
        };
        Ok(Self { query, captures })
    }

    fn found<'tree>(&self, found: &QueryMatch<'_, 'tree>) -> Found<'tree> {
        let mut out = Found::default();
        for capture in found.captures() {
            let node = Some(capture.node);
            match capture.index {
                i if i == self.captures.name => out.name = node,
                i if i == self.captures.visibility => out.visibility = node,
                i if i == self.captures.argument => out.argument = node,
                i => {
                    let record = record_kind(self.query.capture_names()[i as usize]);
                    out.record = record.map(|record| (record, capture.node));
                }
            }
        }
        out
    }
}

impl Language for RustLanguage {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn extract(&self, source: &str) -> Result<Extract, ExtractError> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|err| ExtractError::Grammar(err.to_string()))?;
        let tree = parser.parse(source, None).ok_or(ExtractError::Parse)?;
        let src = source.as_bytes();
        let mut out = Extract::default();
        let contexts = node_contexts(tree.root_node(), src);
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), src);
        while let Some(found) = matches.next() {
            record(self.found(found), src, &contexts, &mut out);
        }
        scan::scan(tree.root_node(), src, &mut out);
        Ok(out)
    }
}

fn record_kind(capture: &str) -> Option<Record> {
    let kind = match capture {
        "function" => ItemKind::Function,
        "struct" => ItemKind::Struct,
        "enum" => ItemKind::Enum,
        "trait" => ItemKind::Trait,
        "module" => ItemKind::Module,
        "use" => return Some(Record::Use),
        _ => return None,
    };
    Some(Record::Item(kind))
}

struct NodeContext {
    scope: Rc<String>,
    in_cfg_test: bool,
    owner: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    Module,
    ModHeader,
    TypeDeclList,
    Body,
}

struct ScopeFrame {
    scope: Rc<String>,
    in_cfg_test: bool,
    placement: Placement,
    owner: Option<String>,
    fn_depth: usize,
}

impl ScopeFrame {
    fn with_placement(&self, placement: Placement) -> Rc<Self> {
        Rc::new(Self {
            scope: Rc::clone(&self.scope),
            in_cfg_test: self.in_cfg_test,
            placement,
            owner: self.owner.clone(),
            fn_depth: self.fn_depth,
        })
    }
}

fn node_contexts(root: Node<'_>, src: &[u8]) -> HashMap<usize, NodeContext> {
    let mut out = HashMap::new();
    let top = Rc::new(ScopeFrame {
        scope: Rc::new(String::new()),
        in_cfg_test: false,
        placement: Placement::Module,
        owner: None,
        fn_depth: 0,
    });
    let mut stack = vec![(root, top)];
    while let Some((node, frame)) = stack.pop() {
        record_node_context(node, &frame, &mut out);
        if let Some(child_frame) = step_scope_frame(node, &frame, src) {
            push_named_children(node, &child_frame, &mut stack);
        }
    }
    out
}

fn push_named_children<'t, T>(node: Node<'t>, state: &Rc<T>, stack: &mut Vec<(Node<'t>, Rc<T>)>) {
    let mut cursor = node.walk();
    let children: Vec<Node<'t>> = node.named_children(&mut cursor).collect();
    for child in children.into_iter().rev() {
        stack.push((child, Rc::clone(state)));
    }
}

fn record_node_context(node: Node<'_>, frame: &ScopeFrame, out: &mut HashMap<usize, NodeContext>) {
    let Some(owner) = eligible_owner(node.kind(), frame) else {
        return;
    };
    out.insert(
        node.id(),
        NodeContext {
            scope: Rc::clone(&frame.scope),
            in_cfg_test: frame.in_cfg_test,
            owner,
        },
    );
}

fn eligible_owner(kind: &str, frame: &ScopeFrame) -> Option<Option<String>> {
    if frame.placement == Placement::Module {
        return Some(None);
    }
    if kind == "function_item"
        && frame.placement == Placement::Body
        && (1..=MAX_NESTED_FN_DEPTH).contains(&frame.fn_depth)
    {
        return Some(frame.owner.clone());
    }
    None
}

const SKIP_SCOPE_DESCENDANTS: [&str; 6] = [
    "trait_item",
    "macro_definition",
    "token_tree",
    "struct_item",
    "enum_item",
    "use_declaration",
];

fn step_scope_frame(node: Node<'_>, frame: &Rc<ScopeFrame>, src: &[u8]) -> Option<Rc<ScopeFrame>> {
    let kind = node.kind();
    if SKIP_SCOPE_DESCENDANTS.contains(&kind) {
        return None;
    }
    if matches!(kind, "source_file" | "ERROR") {
        return Some(Rc::clone(frame));
    }
    step_non_leaf_frame(node, kind, frame, src)
}

fn step_non_leaf_frame(
    node: Node<'_>,
    kind: &str,
    frame: &Rc<ScopeFrame>,
    src: &[u8],
) -> Option<Rc<ScopeFrame>> {
    match kind {
        "mod_item" => step_mod_frame(node, frame, src),
        "impl_item" => step_impl_frame(node, frame, src),
        "declaration_list" => Some(frame.with_placement(decl_placement(frame.placement))),
        "function_item" => step_fn_frame(node, frame, src),
        _ => Some(clear_direct_scope(frame)),
    }
}

fn decl_placement(parent: Placement) -> Placement {
    if parent == Placement::ModHeader {
        Placement::Module
    } else {
        Placement::TypeDeclList
    }
}

fn clear_direct_scope(frame: &Rc<ScopeFrame>) -> Rc<ScopeFrame> {
    if frame.placement == Placement::Body {
        Rc::clone(frame)
    } else {
        frame.with_placement(Placement::Body)
    }
}

fn step_mod_frame(node: Node<'_>, frame: &ScopeFrame, src: &[u8]) -> Option<Rc<ScopeFrame>> {
    if frame.placement != Placement::Module {
        return None;
    }
    let name = compact(node.child_by_field_name("name")?, src);
    Some(Rc::new(ScopeFrame {
        scope: Rc::new(join_scope(&frame.scope, &name)),
        in_cfg_test: frame.in_cfg_test || attrs::cfg_test(node, src),
        placement: Placement::ModHeader,
        owner: None,
        fn_depth: 0,
    }))
}

fn step_impl_frame(node: Node<'_>, frame: &ScopeFrame, src: &[u8]) -> Option<Rc<ScopeFrame>> {
    if frame.placement != Placement::Module {
        return None;
    }
    let ty = node.child_by_field_name("type")?;
    Some(Rc::new(ScopeFrame {
        scope: Rc::clone(&frame.scope),
        in_cfg_test: frame.in_cfg_test,
        placement: Placement::Body,
        owner: Some(scan::owner_name(ty, src)),
        fn_depth: 0,
    }))
}

fn step_fn_frame(node: Node<'_>, frame: &ScopeFrame, src: &[u8]) -> Option<Rc<ScopeFrame>> {
    let next_depth = frame.fn_depth + 1;
    if next_depth > MAX_NESTED_FN_DEPTH {
        return None;
    }
    let name = compact(node.child_by_field_name("name")?, src);
    Some(Rc::new(ScopeFrame {
        scope: Rc::clone(&frame.scope),
        in_cfg_test: frame.in_cfg_test,
        placement: Placement::Body,
        owner: Some(join_owner(frame.owner.as_deref(), &name)),
        fn_depth: next_depth,
    }))
}

pub(crate) fn join_scope(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}::{name}")
    }
}

fn join_owner(owner: Option<&str>, name: &str) -> String {
    match owner {
        Some(prev) => format!("{prev}::{name}"),
        None => name.to_owned(),
    }
}

fn record(found: Found<'_>, src: &[u8], contexts: &HashMap<usize, NodeContext>, out: &mut Extract) {
    let Some((record, node)) = found.record else {
        return;
    };
    let Some(ctx) = contexts.get(&node.id()) else {
        return;
    };
    let visibility = found
        .visibility
        .map_or(Visibility::Private, |node| visibility(node, src));
    let start_line = start_line(node);
    match (record, found.name, found.argument) {
        (Record::Item(kind), Some(name), _) => {
            let detail = detail::detail(node, src);
            out.items.push(Item {
                kind,
                name: compact(name, src),
                visibility,
                start_line,
                end_line: detail.end_line,
                signature: detail.signature,
                doc_summary: detail.doc_summary,
                is_test: attrs::is_test(node, src, ctx.in_cfg_test),
            });
            out.item_scopes.push((*ctx.scope).clone());
            out.item_owners.push(ctx.owner.clone());
        }
        (Record::Use, _, Some(argument)) => {
            imports::flatten(argument, src, &visibility, start_line, &mut out.imports);
            out.import_scopes
                .resize(out.imports.len(), (*ctx.scope).clone());
        }
        _ => {}
    }
}

/// Reads a `visibility_modifier`: bare `pub`, `pub(<target>)`, or
/// `pub(in <path>)`.
fn visibility(node: Node<'_>, src: &[u8]) -> Visibility {
    let Some(target) = node.named_child(0) else {
        return Visibility::Public;
    };
    let mut cursor = node.walk();
    let has_in = node.children(&mut cursor).any(|child| child.kind() == "in");
    let target_text = compact(target, src);
    match (has_in, target.kind()) {
        (false, "crate") => Visibility::Crate,
        (false, _) => Visibility::Restricted(format!("pub({target_text})")),
        (true, _) => Visibility::Restricted(format!("pub(in {target_text})")),
    }
}

fn start_line(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().row).map_or(u32::MAX, |row| row.saturating_add(1))
}

/// Node text with all whitespace removed.
fn compact(node: Node<'_>, src: &[u8]) -> String {
    node.utf8_text(src)
        .unwrap_or_default()
        .split_whitespace()
        .collect()
}
