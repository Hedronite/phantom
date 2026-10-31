//! The contract between the store and a call resolver (SPEC-w3 G0c,
//! SPEC-w4 G0a, SPEC-w7 G0a).
//!
//! The store fills a [`ResolveInput`] from its rows. A resolver reads it and
//! returns a [`Graph`]: call [`Edge`]s with their [`ResolveCounts`], and
//! beside them the `import` and `impl` [`Link`]s. Nothing here resolves
//! anything. Ids are the store's row ids. Paths are repo-relative and
//! `/`-separated.

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
    /// Which workspace libs each package may call by crate name.
    pub deps: Vec<Dep>,
}

/// Package `package` may call the lib of workspace package `on`, written
/// as `crate_name`. A dev-dependency is callable from test targets only
/// (SPEC-w11 G1b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The calling package's name.
    pub package: String,
    /// The called package's name.
    pub on: String,
    /// The first path segment a caller writes: `on` with `-` turned to `_`.
    pub crate_name: String,
    /// True for a `[dev-dependencies]` entry.
    pub dev: bool,
}

/// One compilation target of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: Id,
    /// The package name from its `Cargo.toml`.
    pub package: String,
    /// The target name: the package name for a lib, the bin's name, or a
    /// test's file or directory name.
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
    /// An integration test: `tests/<name>.rs` or `tests/<dir>/main.rs`.
    Test,
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
    /// The stored signature text, as [`crate::Item::signature`] gives it.
    pub signature: Option<String>,
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
    /// The stored `use` reference.
    pub id: Id,
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
    /// The receiver text of a method call as stored, such as `self`; none
    /// for other kinds.
    pub receiver: Option<String>,
    /// The receiver's type hint as stored, as
    /// [`crate::Reference::receiver_type`] gives it; none without one.
    pub receiver_type: Option<String>,
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
    /// `self.m()` in an impl method to the one method `m` of the impl's
    /// self type.
    SelfMethod,
    /// `Self::name()` in an impl method to the one method `name` of the
    /// impl's self type.
    SelfAssoc,
    /// The first segment is the crate name of a workspace lib the caller's
    /// package may call.
    CratePath,
    /// `x.m()` where `x` has a type hint naming one workspace struct or
    /// enum, to the one method `m` of that type (SPEC-w10 G2a).
    TypedMethod,
    /// `x.m()` with no placed hint, to the one workspace method named `m`
    /// (SPEC-w10 G3a).
    UniqueMethod,
}

impl Reason {
    /// The store's `edges.reason` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathExact => "path-exact",
            Self::Local => "local",
            Self::UseAlias => "use-alias",
            Self::Assoc => "assoc",
            Self::SelfMethod => "self-method",
            Self::SelfAssoc => "self-assoc",
            Self::CratePath => "crate-path",
            Self::TypedMethod => "typed-method",
            Self::UniqueMethod => "unique-method",
        }
    }
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
    /// Method calls on a receiver other than `self` that got no edge.
    pub method_calls: u64,
    /// Macro references, counted and never resolved.
    pub macros: u64,
}

/// Everything one resolve pass places: the call edges and their counts,
/// and the `import` and `impl` links beside them. Links add nothing to
/// the counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    /// Call edges, in ref order.
    pub calls: Vec<Edge>,
    pub counts: ResolveCounts,
    /// `import` links in `use` order, then `impl` links in method order.
    pub links: Vec<Link>,
}

/// One resolved edge of a kind other than `call`.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub kind: LinkKind,
    /// The importing module, or the implementing type.
    pub source: Node,
    /// The imported module or symbol, or the implemented trait.
    pub target: Node,
    /// In `0.0..=1.0`: the confidence of the path tier that placed it.
    pub confidence: f64,
    pub reason: LinkReason,
    /// The [`UseBinding`] of an `import` link; none for an `impl` link.
    pub ref_id: Option<Id>,
}

/// The kinds of [`Link`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// A non-glob `use` leaf, from the module it sits in to the one
    /// workspace module or symbol its path names.
    Import,
    /// A struct or enum to the one workspace trait it implements.
    Impl,
}

impl LinkKind {
    /// The store's `edges.kind` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Impl => "impl",
        }
    }
}

/// One end of a [`Link`]: a module row or a symbol row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    Module(Id),
    Symbol(Id),
}

impl Node {
    /// The row id.
    pub fn id(self) -> Id {
        match self {
            Self::Module(id) | Self::Symbol(id) => id,
        }
    }

    /// The store's `edges.src_kind` or `edges.dst_kind` text.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Module(_) => "module",
            Self::Symbol(_) => "symbol",
        }
    }
}

/// Why a resolver placed a [`Link`]. An `import` link carries the path
/// tier of its `use` path, named as for a call [`Reason`]; an `impl` link
/// is always `ImplTrait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkReason {
    PathExact,
    Local,
    UseAlias,
    CratePath,
    /// An `impl Trait for Type`, whose trait path resolved by any tier.
    ImplTrait,
}

impl LinkReason {
    /// The store's `edges.reason` text, the same text a call edge of the
    /// same tier carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathExact => "path-exact",
            Self::Local => "local",
            Self::UseAlias => "use-alias",
            Self::CratePath => "crate-path",
            Self::ImplTrait => "impl-trait",
        }
    }
}
