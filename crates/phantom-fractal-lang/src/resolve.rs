//! The contract between the store and a call resolver (SPEC-w3 G0c).
//!
//! The store fills a [`ResolveInput`] from its rows. A resolver reads it and
//! returns [`Edge`]s and [`ResolveCounts`]. Nothing here resolves anything.
//! Ids are the store's row ids. Paths are repo-relative and `/`-separated.

use crate::{ItemKind, RefKind};

/// A store row id.
pub type Id = i64;

/// Everything a resolver reads for one repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveInput {
    pub targets: Vec<Target>,
    pub modules: Vec<Module>,
    pub symbols: Vec<Symbol>,
    pub uses: Vec<UseBinding>,
    pub refs: Vec<CallRef>,
}

/// One compilation target of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: Id,
    /// The package name from its `Cargo.toml`.
    pub package: String,
    /// The target name: the package name for a lib, or the bin's name.
    pub name: String,
    pub kind: TargetKind,
    /// The crate root file, such as `src/lib.rs`.
    pub root: String,
}

/// What a [`Target`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    Lib,
    Bin,
}

/// One module of a target: a file module or an inline `mod`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub id: Id,
    pub target: Id,
    /// The parent module, or none for the crate root.
    pub parent: Option<Id>,
    /// The module path from the crate root, such as `crate::a::b`.
    pub path: String,
    /// The file that holds the module's body.
    pub file: String,
    /// The inline-module path within `file`: empty for a file module.
    pub scope: String,
}

/// A declared item or method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub id: Id,
    pub kind: SymbolKind,
    pub name: String,
    /// For a method, the `impl` self type's last path segment.
    pub owner: Option<String>,
    /// For a method, the `impl` self type as written.
    pub owner_path: Option<String>,
    /// For a method in `impl Trait for T`, the trait as written. Inherent
    /// methods have none.
    pub trait_name: Option<String>,
    /// The inline-module path within `file`.
    pub scope: String,
    /// The module the symbol is declared in, or none for an orphan file.
    pub module: Option<Id>,
    pub file: String,
}

/// The kinds of [`Symbol`]: every [`ItemKind`], plus methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Module,
    Method,
}

impl From<ItemKind> for SymbolKind {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Function => Self::Function,
            ItemKind::Struct => Self::Struct,
            ItemKind::Enum => Self::Enum,
            ItemKind::Trait => Self::Trait,
            ItemKind::Module => Self::Module,
        }
    }
}

/// One `use` leaf, bound in a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseBinding {
    /// The module the `use` is declared in, or none for an orphan file.
    pub module: Option<Id>,
    /// The inline-module path within the module's file.
    pub scope: String,
    pub file: String,
    /// The path as written; a glob's path stops before `*`.
    pub path: String,
    pub alias: Option<String>,
    pub glob: bool,
}

/// One stored call reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRef {
    pub id: Id,
    /// The symbol of the enclosing function, or none outside any function.
    pub from: Option<Id>,
    /// The module the call sits in, or none for an orphan file.
    pub module: Option<Id>,
    /// The inline-module path within `file`.
    pub scope: String,
    pub file: String,
    pub kind: RefKind,
    /// The callee as written, whitespace removed.
    pub text: String,
    pub shadowed: bool,
}

/// One resolved call edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// The [`CallRef`] the edge resolves.
    pub ref_id: Id,
    /// The calling symbol.
    pub source: Id,
    /// The called symbol.
    pub target: Id,
    /// In `0.0..=1.0`.
    pub confidence: f64,
    pub reason: Reason,
}

/// Why a resolver placed an [`Edge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// A `crate::`, `self::`, or `super::` path to one item or method.
    PathExact,
    /// The first segment is declared in the call's module.
    Local,
    /// The first segment is bound by a non-glob `use` or its alias.
    UseAlias,
    /// `Type::name` to the one inherent method of that type.
    Assoc,
}

/// What one resolve pass did with the call references it saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolveCounts {
    /// Edges placed.
    pub edges: u64,
    /// Calls into the workspace that no tier placed.
    pub unresolved: u64,
    /// Calls whose first segment is `std`, `core`, `alloc`, or not found.
    pub external: u64,
    /// Calls outside any function.
    pub no_source: u64,
    /// Method-call references, counted and never resolved.
    pub method_calls: u64,
    /// Macro references, counted and never resolved.
    pub macros: u64,
}
