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
mod receivers;
mod resolve;
mod scan;
mod std_methods;

use phantom_fractal_lang::{Extract, ExtractError, Item, ItemKind, Language, Visibility};

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
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), src);
        while let Some(found) = matches.next() {
            record(self.found(found), src, &mut out);
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

fn record(found: Found<'_>, src: &[u8], out: &mut Extract) {
    let Some((record, node)) = found.record else {
        return;
    };
    if !at_module_scope(node) {
        return;
    }
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
                is_test: attrs::is_test(node, src),
            });
            out.item_scopes.push(scope_of(node, src));
        }
        (Record::Use, _, Some(argument)) => {
            imports::flatten(argument, src, &visibility, start_line, &mut out.imports);
            let scope = scope_of(node, src);
            out.import_scopes.resize(out.imports.len(), scope);
        }
        _ => {}
    }
}

/// The inline-module path around `node`, outermost first, joined with `::`.
fn scope_of(node: Node<'_>, src: &[u8]) -> String {
    let mut names = Vec::new();
    let mut parent = node.parent();
    while let Some(ancestor) = parent {
        if ancestor.kind() == "mod_item"
            && let Some(name) = ancestor.child_by_field_name("name")
        {
            names.push(compact(name, src));
        }
        parent = ancestor.parent();
    }
    names.reverse();
    names.join("::")
}

/// True for a declaration in the file body or an inline `mod` body.
/// `ERROR` wrappers from parser recovery do not change the scope.
fn at_module_scope(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(error) = parent.filter(Node::is_error) {
        parent = error.parent();
    }
    match parent {
        Some(parent) if parent.kind() == "source_file" => true,
        Some(parent) if parent.kind() == "declaration_list" => parent
            .parent()
            .is_some_and(|owner| owner.kind() == "mod_item"),
        _ => false,
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
