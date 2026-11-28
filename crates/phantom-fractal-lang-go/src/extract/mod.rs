//! Go syntax extractor implementing [`phantom_fractal_lang::Language`].

mod detail;
mod items;
mod scan;

use phantom_fractal_lang::{Extract, ExtractError, Language};
use tree_sitter::{Node, Parser};

pub(crate) use detail::bare_type_name;
pub use items::{LinknameDirective, default_import_qualifier, parse_linknames};

/// Go language parser and extractor backed by `tree-sitter-go`.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoLanguage;

impl GoLanguage {
    /// Creates a new Go language extractor.
    pub fn new() -> Self {
        Self
    }
}

impl Language for GoLanguage {
    fn name(&self) -> &'static str {
        "go"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn qualifier_separator(&self) -> &'static str {
        "."
    }

    fn has_parse_errors(&self, source: &str) -> bool {
        let Ok(tree) = parse_tree(source) else {
            return true;
        };
        node_has_parse_errors(tree.root_node())
    }

    fn extract(&self, source: &str) -> Result<Extract, ExtractError> {
        let tree = parse_tree(source)?;
        let src = source.as_bytes();
        let mut out = Extract::default();
        let decls = items::collect_top_level(tree.root_node(), src, &mut out);
        scan::scan_bodies(&decls, src, &mut out);
        Ok(out)
    }
}

fn parse_tree(source: &str) -> Result<tree_sitter::Tree, ExtractError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|err| ExtractError::Grammar(err.to_string()))?;
    parser.parse(source, None).ok_or(ExtractError::Parse)
}

fn node_has_parse_errors(root: Node<'_>) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            return true;
        }
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            stack.push(child);
        }
    }
    false
}
