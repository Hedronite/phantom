//! Language trait for Phantom extractors.
//!
//! An extractor turns one source file into an [`Extract`]: the items it
//! declares, the imports it names, and the calls it makes. Records hold
//! source text only. Nothing here resolves a path to a target, links a caller
//! to a callee, or scores confidence. The [`resolve`] types are the contract
//! between the store and a resolver, with no logic of their own.
#![forbid(unsafe_code)]

pub mod resolve;

use std::error::Error;
use std::fmt;

pub use resolve::{
    CallRef, Edge, Id, Module, Reason, ResolveCounts, ResolveInput, Symbol, SymbolKind, Target,
    TargetKind, UseBinding,
};

/// A language Phantom can extract.
pub trait Language {
    /// Short lowercase name, such as `rust`.
    fn name(&self) -> &'static str;

    /// File extensions this language claims, without the leading dot.
    fn extensions(&self) -> &'static [&'static str];

    /// Extracts items, imports, call references, methods, and module
    /// declarations from one file's source text.
    ///
    /// Syntax errors do not fail extraction. Records outside the broken
    /// region still come back.
    fn extract(&self, source: &str) -> Result<Extract, ExtractError>;
}

/// Everything extracted from one source file, in source order.
///
/// A scope is the inline-module path of a record within its file, joined
/// with `::`: empty at file level, `tests` inside `mod tests`, and `a::b`
/// inside `mod a { mod b { .. } }`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extract {
    pub items: Vec<Item>,
    pub imports: Vec<Import>,
    /// The scope of each item, parallel to `items`.
    pub item_scopes: Vec<String>,
    /// The scope of each import, parallel to `imports`.
    pub import_scopes: Vec<String>,
    /// Calls, method calls, and macro invocations, by line then column.
    pub refs: Vec<Reference>,
    /// Every `fn` inside an `impl` block at module scope.
    pub methods: Vec<Method>,
    /// Every `mod` declaration at module scope, inline or not.
    pub mod_decls: Vec<ModDecl>,
}

/// One call site, as written. Nothing is resolved.
///
/// Calls inside a macro's token tree are not visible: macros are never
/// expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub kind: RefKind,
    /// The callee with whitespace removed and generic arguments kept:
    /// `a::f`, `Vec::<u8>::new`, `collect::<Vec<_>>` for a method, or the
    /// macro path without `!`.
    pub text: String,
    /// The last path segment of the callee, without generic arguments.
    pub last_segment: String,
    /// 1-based line of the callee's first token.
    pub line: u32,
    /// 1-based byte column of the callee's first token.
    pub col: u32,
    /// The inline-module path the call sits in.
    pub scope: String,
    /// The innermost enclosing `fn`, or none outside any function. A call
    /// in a closure belongs to the closure's enclosing function.
    pub from: Option<Caller>,
    /// The receiver expression of a [`RefKind::MethodCall`], whitespace
    /// removed.
    pub receiver: Option<String>,
    /// True for a single-segment [`RefKind::Call`] whose name is bound
    /// anywhere in the enclosing function: by `let`, a parameter, a closure
    /// parameter, a `match` arm, a `for`, `if let`, or `while let` pattern,
    /// or a local item or `use`. Conservative: a binding after the call, or
    /// in another branch, still counts.
    pub shadowed: bool,
}

/// What kind of call a [`Reference`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// A path or identifier in function position: `f()`, `a::f()`,
    /// `Type::new()`.
    Call,
    /// A call on a value: `x.m()`.
    MethodCall,
    /// A macro invocation, recorded by name only.
    Macro,
}

/// The function a [`Reference`] sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub name: String,
    /// The `impl` self type's last path segment for a method, without
    /// generics, or the trait name for a trait's default body.
    pub owner: Option<String>,
}

/// A `fn` inside an `impl` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    /// The self type's last path segment, generics removed: `Ledger` for
    /// `impl<T> a::Ledger<T>`.
    pub owner: String,
    /// The self type as written, whitespace removed.
    pub owner_path: String,
    /// The trait as written for `impl Trait for T`, or none.
    pub trait_name: Option<String>,
    pub name: String,
    pub visibility: Visibility,
    /// The inline-module path of the `impl` block.
    pub scope: String,
    /// 1-based line of the `fn`'s first token, after any attributes.
    pub start_line: u32,
}

/// A `mod` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDecl {
    pub name: String,
    /// The inline-module path the declaration sits in.
    pub scope: String,
    /// True for `mod a { .. }`, false for `mod a;`.
    pub inline: bool,
    /// The value of a `#[path = "..."]` attribute on the declaration.
    pub path: Option<String>,
    /// 1-based line of the `mod` keyword or its visibility.
    pub line: u32,
}

/// A declared item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub name: String,
    pub visibility: Visibility,
    /// 1-based line of the item's first token, after any attributes.
    pub start_line: u32,
}

/// The kinds of item this slice extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Function,
    Struct,
    Enum,
    Trait,
    Module,
}

impl ItemKind {
    /// Stable lowercase name, for storage and display.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Module => "module",
        }
    }
}

/// One imported path, as written.
///
/// A grouped import yields one record per leaf. `use a::{b, c::*}` gives
/// `a::b` and a glob over `a::c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Path text with whitespace removed. A glob's path stops before `*`.
    pub path: String,
    /// The name after `as`, if any. `as _` keeps `_`.
    pub alias: Option<String>,
    /// True for a `*` import.
    pub glob: bool,
    pub visibility: Visibility,
    /// 1-based line of the import declaration.
    pub start_line: u32,
}

/// Declared visibility.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// No modifier.
    Private,
    /// `pub`.
    Public,
    /// `pub(crate)`.
    Crate,
    /// Any other restriction, in canonical spacing, such as `pub(super)`,
    /// `pub(self)`, or `pub(in crate::a)`.
    Restricted(String),
}

/// Why extraction could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtractError {
    /// The grammar or a query failed to load.
    Grammar(String),
    /// The parser returned no tree.
    Parse,
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Grammar(detail) => write!(f, "grammar failed to load: {detail}"),
            Self::Parse => f.write_str("parser returned no tree"),
        }
    }
}

impl Error for ExtractError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_kind_names_are_distinct() {
        let kinds = [
            ItemKind::Function,
            ItemKind::Struct,
            ItemKind::Enum,
            ItemKind::Trait,
            ItemKind::Module,
        ];
        let mut names: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), kinds.len());
    }

    #[test]
    fn extract_error_displays_its_cause() {
        let grammar = ExtractError::Grammar("bad query".to_owned());
        assert_eq!(grammar.to_string(), "grammar failed to load: bad query");
        assert_eq!(ExtractError::Parse.to_string(), "parser returned no tree");
    }
}
