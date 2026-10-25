//! Flattens one `use` tree into an [`Import`] per leaf.
//!
//! Paths are joined as text. `a::{self}` becomes `a`. Nothing is resolved.

use phantom_fractal_lang::{Import, Visibility};
use tree_sitter::Node;

use crate::compact;

pub(crate) fn flatten(
    argument: Node<'_>,
    src: &[u8],
    visibility: &Visibility,
    start_line: u32,
    out: &mut Vec<Import>,
) {
    let mut flatten = Flatten {
        src,
        visibility,
        start_line,
        out,
    };
    flatten.walk(argument, "");
}

/// The fields every leaf of one `use` declaration shares.
struct Flatten<'a> {
    src: &'a [u8],
    visibility: &'a Visibility,
    start_line: u32,
    out: &'a mut Vec<Import>,
}

impl Flatten<'_> {
    fn walk(&mut self, node: Node<'_>, prefix: &str) {
        match node.kind() {
            "use_list" => self.walk_list(node, prefix),
            "scoped_use_list" => {
                let prefix = match node.child_by_field_name("path") {
                    Some(path) => join(prefix, &compact(path, self.src)),
                    None => prefix.to_owned(),
                };
                if let Some(list) = node.child_by_field_name("list") {
                    self.walk_list(list, &prefix);
                }
            }
            "use_as_clause" => {
                let path = self.field(node, "path").unwrap_or_default();
                let alias = self.field(node, "alias");
                self.push(join(prefix, &path), alias, false);
            }
            "use_wildcard" => {
                let path = match node.named_child(0) {
                    Some(path) => join(prefix, &compact(path, self.src)),
                    None => prefix.to_owned(),
                };
                self.push(path, None, true);
            }
            _ => {
                let path = join(prefix, &compact(node, self.src));
                self.push(path, None, false);
            }
        }
    }

    fn walk_list(&mut self, list: Node<'_>, prefix: &str) {
        let mut cursor = list.walk();
        for child in list.named_children(&mut cursor) {
            if !child.is_extra() {
                self.walk(child, prefix);
            }
        }
    }

    fn field(&self, node: Node<'_>, name: &str) -> Option<String> {
        node.child_by_field_name(name)
            .map(|child| compact(child, self.src))
    }

    fn push(&mut self, path: String, alias: Option<String>, glob: bool) {
        self.out.push(Import {
            path,
            alias,
            glob,
            visibility: self.visibility.clone(),
            start_line: self.start_line,
        });
    }
}

fn join(prefix: &str, segment: &str) -> String {
    match (prefix, segment) {
        ("", _) => segment.to_owned(),
        (_, "self") => prefix.to_owned(),
        _ => format!("{prefix}::{segment}"),
    }
}
