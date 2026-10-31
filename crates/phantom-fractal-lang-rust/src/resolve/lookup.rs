//! Name lookup over the module tree for [`super::resolve`].
//!
//! Inside a module, a local item or child module beats a `use` binding. A
//! name only a glob could supply is not looked up. Inline modules nest by
//! scope, and `super` moves one module up. `self.m()` and `Self::m()` look
//! up the methods of the calling impl's self type (SPEC-w4 G1). A first
//! segment that nothing in scope binds may name a workspace lib the
//! caller's target may call (SPEC-w4 G2b). `use` paths and `impl` trait
//! paths resolve by the same walk (SPEC-w7 G0a).

use std::collections::HashMap;

use phantom_fractal_lang::{
    CallRef, Id, LinkReason, Module, Node, Reason, ResolveInput, Symbol, SymbolKind, TargetKind,
    UseBinding,
};

use super::segments;
use crate::STD_METHOD_NAMES;

/// How deep `use` chains are followed before a lookup gives up.
const MAX_DEPTH: u32 = 8;

/// First segments that name a standard crate.
const STANDARD_CRATES: [&str; 3] = ["std", "core", "alloc"];

/// What [`Index::place`] decided for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placed {
    Edge(Id, Reason),
    Unresolved,
    External,
}

/// A name's meaning: a module row or a symbol row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Entity {
    Module(Id),
    Symbol(Id),
}

/// Why a walk stopped without an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Miss {
    Unresolved,
    External,
}

/// How the first segment of a path was bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    PathExact,
    Local,
    UseAlias,
    CratePath,
}

/// A walk's result: the entities the last segment names, how the first
/// segment was bound, whether the last hop went from a type to one of its
/// methods, and whether the walk entered another crate by name, directly or
/// through a `use`.
struct Walked {
    entities: Vec<Entity>,
    tier: Tier,
    method_hop: bool,
    crossed: bool,
}

impl Walked {
    /// The tier that placed the path: `CratePath` once the walk entered
    /// another crate by name, else how the first segment was bound.
    fn path_tier(&self) -> Tier {
        if self.crossed {
            Tier::CratePath
        } else {
            self.tier
        }
    }
}

impl Tier {
    /// The call reason of this tier.
    pub(crate) fn reason(self) -> Reason {
        match self {
            Self::PathExact => Reason::PathExact,
            Self::Local => Reason::Local,
            Self::UseAlias => Reason::UseAlias,
            Self::CratePath => Reason::CratePath,
        }
    }

    /// The `import` link reason of this tier, the same text as
    /// [`Self::reason`].
    pub(crate) fn link_reason(self) -> LinkReason {
        match self {
            Self::PathExact => LinkReason::PathExact,
            Self::Local => LinkReason::Local,
            Self::UseAlias => LinkReason::UseAlias,
            Self::CratePath => LinkReason::CratePath,
        }
    }
}

/// What a name binds in one module: the entities, whether they are
/// declared there, and whether a `use` reached them through another crate.
struct Found {
    entities: Vec<Entity>,
    local: bool,
    crossed: bool,
}

pub(crate) struct Index<'a> {
    modules: HashMap<Id, &'a Module>,
    symbols: HashMap<Id, &'a Symbol>,
    /// `(file, scope)` to the module that holds it.
    places: HashMap<(&'a str, &'a str), Id>,
    /// Target to its root module.
    roots: HashMap<Id, Id>,
    /// `(parent, name)` to child modules.
    children: HashMap<(Id, &'a str), Vec<Id>>,
    /// `(module, name)` to the non-method symbols declared there.
    items: HashMap<(Id, &'a str), Vec<Id>>,
    /// Module to the `use` bindings declared in it.
    uses: HashMap<Id, Vec<&'a UseBinding>>,
    /// Method name to `(method, owner type)` for every inherent method
    /// whose owner resolves to one type.
    inherent: HashMap<&'a str, Vec<(Id, Id)>>,
    /// `(struct or enum, name)` to every method, inherent or trait impl,
    /// whose owner resolves to that one type.
    by_type: HashMap<(Id, &'a str), Vec<Id>>,
    /// Method name to every method of that name in the workspace.
    by_name: HashMap<&'a str, Vec<Id>>,
    /// Target to the crate names it may call, each to that lib's root
    /// module, or none when two libs share the name.
    crates: HashMap<Id, HashMap<String, Option<Id>>>,
}

impl<'a> Index<'a> {
    pub(crate) fn new(input: &'a ResolveInput) -> Self {
        let mut index = Index {
            modules: input.modules.iter().map(|m| (m.id, m)).collect(),
            symbols: input.symbols.iter().map(|s| (s.id, s)).collect(),
            places: HashMap::new(),
            roots: HashMap::new(),
            children: HashMap::new(),
            items: HashMap::new(),
            uses: HashMap::new(),
            inherent: HashMap::new(),
            by_type: HashMap::new(),
            by_name: HashMap::new(),
            crates: HashMap::new(),
        };
        for module in &input.modules {
            index
                .places
                .entry((module.file.as_str(), module.scope.as_str()))
                .or_insert(module.id);
            match module.parent {
                None => {
                    index.roots.entry(module.target).or_insert(module.id);
                }
                Some(parent) => index
                    .children
                    .entry((parent, last_segment(&module.path)))
                    .or_default()
                    .push(module.id),
            }
        }
        for symbol in &input.symbols {
            if symbol.kind == SymbolKind::Method {
                index
                    .by_name
                    .entry(symbol.name.as_str())
                    .or_default()
                    .push(symbol.id);
                continue;
            }
            if let Some(module) = index.place_of(&symbol.file, &symbol.scope) {
                index
                    .items
                    .entry((module, symbol.name.as_str()))
                    .or_default()
                    .push(symbol.id);
            }
        }
        for binding in &input.uses {
            if let Some(module) = index.place_of(&binding.file, &binding.scope) {
                index.uses.entry(module).or_default().push(binding);
            }
        }
        // Owner paths may start with a crate name, so the names come first.
        index.crates = index.crate_names(input);
        let mut inherent: HashMap<&'a str, Vec<(Id, Id)>> = HashMap::new();
        let mut by_type: HashMap<(Id, &'a str), Vec<Id>> = HashMap::new();
        for method in &input.symbols {
            if method.kind != SymbolKind::Method {
                continue;
            }
            let Some(owner) = index.owner_type(method) else {
                continue;
            };
            if method.trait_name.is_none() {
                inherent
                    .entry(method.name.as_str())
                    .or_default()
                    .push((method.id, owner));
            }
            if index.is_data_type(owner) {
                by_type
                    .entry((owner, method.name.as_str()))
                    .or_default()
                    .push(method.id);
            }
        }
        index.inherent = inherent;
        index.by_type = by_type;
        index
    }

    /// The crate names each target may call: every lib its package has a
    /// `deps` entry for, and, for a bin or test, its own package's lib. A
    /// dev entry counts for a test only (SPEC-w11 G1b). A lib never names
    /// itself.
    fn crate_names(&self, input: &ResolveInput) -> HashMap<Id, HashMap<String, Option<Id>>> {
        let lib_root = |package: &str| {
            input
                .targets
                .iter()
                .find(|t| t.kind == TargetKind::Lib && t.package == package)
                .and_then(|t| self.roots.get(&t.id).copied())
        };
        let mut out: HashMap<Id, HashMap<String, Option<Id>>> = HashMap::new();
        for target in &input.targets {
            let mut callable: Vec<(String, Id)> = input
                .deps
                .iter()
                .filter(|dep| dep.package == target.package && dep.on != target.package)
                .filter(|dep| !dep.dev || target.kind == TargetKind::Test)
                .filter_map(|dep| Some((dep.crate_name.clone(), lib_root(&dep.on)?)))
                .collect();
            if matches!(target.kind, TargetKind::Bin | TargetKind::Test)
                && let Some(root) = lib_root(&target.package)
            {
                callable.push((target.package.replace('-', "_"), root));
            }
            let names = out.entry(target.id).or_default();
            for (name, root) in callable {
                let slot = names.entry(name).or_insert(Some(root));
                if *slot != Some(root) {
                    *slot = None;
                }
            }
        }
        out
    }

    /// Places one `Call` reference whose source symbol is `source`.
    pub(crate) fn place(&self, call: &CallRef, source: Id) -> Placed {
        if call.shadowed {
            return Placed::Unresolved;
        }
        let Some(module) = self.place_of(&call.file, &call.scope) else {
            return Placed::Unresolved;
        };
        let Some(segs) = segments(&call.text) else {
            return Placed::Unresolved;
        };
        if let [first, name] = &segs[..]
            && first == "Self"
        {
            return self.place_self(source, name, Reason::SelfAssoc);
        }
        let walked = match self.walk(module, &segs, 0, true) {
            Ok(walked) => walked,
            Err(Miss::External) => return Placed::External,
            Err(Miss::Unresolved) => return Placed::Unresolved,
        };
        let functions: Vec<Id> = walked
            .entities
            .iter()
            .filter_map(|entity| self.function(*entity))
            .collect();
        let [target] = functions[..] else {
            return Placed::Unresolved;
        };
        let reason = match walked.tier {
            Tier::Local | Tier::UseAlias
                if !walked.crossed && segs.len() == 2 && walked.method_hop =>
            {
                Reason::Assoc
            }
            _ => walked.path_tier().reason(),
        };
        Placed::Edge(target, reason)
    }

    /// What a non-glob `use` leaf imports (SPEC-w7 G0a): the module it sits
    /// in, the one workspace module or symbol its path names, and the
    /// path's tier. The path is walked as a call through the binding walks
    /// it, so a link and a call agree on what a `use` means.
    pub(crate) fn import(&self, binding: &UseBinding) -> Option<(Id, Node, Tier)> {
        if binding.glob {
            return None;
        }
        let module = self.place_of(&binding.file, &binding.scope)?;
        // A call reaches a binding one level down from its own lookup.
        let walked = self.use_entities(binding, 1).ok()?;
        let target = match walked.entities[..] {
            [Entity::Module(id)] => Node::Module(id),
            [Entity::Symbol(id)] => Node::Symbol(id),
            _ => return None,
        };
        Some((module, target, walked.path_tier()))
    }

    /// The struct or enum `method` belongs to, by [`Self::owner_of`], and
    /// the one workspace trait its `impl`'s trait path names from the
    /// impl's module, with that path's tier. Inherent methods, and trait
    /// paths that are external, unresolved, or name anything but exactly
    /// one trait, give none.
    pub(crate) fn implemented(&self, method: &Symbol) -> Option<(Id, Id, Tier)> {
        let path = method.trait_name.as_deref()?;
        let ty = self.owner_of(method)?;
        let module = self.place_of(&method.file, &method.scope)?;
        let walked = self.walk(module, &segments(path)?, 0, false).ok()?;
        let traits: Vec<Id> = walked
            .entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Symbol(id) if self.kind(*id) == Some(SymbolKind::Trait) => Some(*id),
                _ => None,
            })
            .collect();
        let [one] = traits[..] else {
            return None;
        };
        Some((ty, one, walked.path_tier()))
    }

    /// Places a call to method `name` of the calling method's own self
    /// type: `self.name()` or `Self::name()` inside `source`.
    ///
    /// `source` must be a method of an `impl` whose self type resolves to
    /// one struct or enum `T`. The candidates are the methods named `name`
    /// whose own impl self type resolves to `T`, inherent or trait impl, in
    /// any target. Exactly one candidate gives an edge, and only when it is
    /// in the caller's target: a trait impl in one crate on another crate's
    /// type never hides that type's inherent method.
    pub(crate) fn place_self(&self, source: Id, name: &str, reason: Reason) -> Placed {
        match self.self_target(source, name) {
            Some(target) => Placed::Edge(target, reason),
            None => Placed::Unresolved,
        }
    }

    fn self_target(&self, source: Id, name: &str) -> Option<Id> {
        let method = self.symbols.get(&source)?;
        if method.kind != SymbolKind::Method {
            return None;
        }
        let ty = self
            .owner_type(method)
            .filter(|ty| self.is_data_type(*ty))?;
        let Some([one]) = self.by_type.get(&(ty, name)).map(Vec::as_slice) else {
            return None;
        };
        let candidate = self.symbols.get(one)?;
        (self.target_of(candidate)? == self.target_of(method)?).then_some(*one)
    }

    /// Places a method call `x.m()` by the receiver hint `hint` (SPEC-w10
    /// G2a): the hint's type, found from the call's module by the type
    /// lookup the `self-method` and `assoc` tiers use, must be exactly one
    /// workspace struct or enum, and the edge goes to its one method named
    /// `m`, inherent or trait impl, as [`Self::owner_of`] pairs them.
    ///
    /// `type:T` names `T`. `ctor:T::f` names `T` when `f` is the one
    /// inherent method `T::f` and its signature returns `Self` or `T`;
    /// `ctor?:T::f` when it returns `Result` or `Option` of `Self` or `T`.
    /// Anything else places nothing.
    pub(crate) fn place_typed(&self, call: &CallRef, hint: &str) -> Option<Id> {
        let module = self.place_of(&call.file, &call.scope)?;
        let name = segments(&call.text)?.pop()?;
        let ty = match hint.split_once(':')? {
            ("type", path) => self.data_type(module, &segments(path)?)?,
            ("ctor", path) => self.constructed(module, path, false)?,
            ("ctor?", path) => self.constructed(module, path, true)?,
            _ => return None,
        };
        let methods: Vec<Id> = self
            .by_type
            .get(&(ty, name.as_str()))?
            .iter()
            .copied()
            .filter(|method| {
                self.symbols
                    .get(method)
                    .and_then(|method| self.owner_of(method))
                    == Some(ty)
            })
            .collect();
        match methods[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    /// Places a method call `x.m()` that has no receiver hint (SPEC-w10
    /// G3a, D6): `m` must be the name of exactly one method in the whole
    /// workspace, and not on [`STD_METHOD_NAMES`].
    pub(crate) fn place_unique(&self, call: &CallRef) -> Option<Id> {
        let name = segments(&call.text)?.pop()?;
        if STD_METHOD_NAMES.binary_search(&name.as_str()).is_ok() {
            return None;
        }
        match self.by_name.get(name.as_str())?.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// The type `T` of a `ctor` hint `T::f`, when `f` is the one inherent
    /// method of `T` so named and returns `Self` or `T`, or with `tried` a
    /// `Result` or `Option` of one.
    fn constructed(&self, module: Id, path: &str, tried: bool) -> Option<Id> {
        let mut segs = segments(path)?;
        let name = segs.pop()?;
        let ty = self.data_type(module, &segs)?;
        let methods: Vec<Id> = self
            .inherent
            .get(name.as_str())?
            .iter()
            .filter(|(_, owner)| *owner == ty)
            .map(|(method, _)| *method)
            .collect();
        let [one] = methods[..] else {
            return None;
        };
        let method = self.symbols.get(&one)?;
        let returned = return_type(method.signature.as_deref()?)?;
        let returned = if tried { ok_type(&returned)? } else { returned };
        if returned == "Self" {
            return Some(ty);
        }
        let home = self.place_of(&method.file, &method.scope)?;
        (self.data_type(home, &segments(&returned)?)? == ty).then_some(ty)
    }

    /// The one struct or enum `segs` names from `module`, by the walk
    /// [`Self::owner_type`] uses.
    fn data_type(&self, module: Id, segs: &[String]) -> Option<Id> {
        let walked = self.walk(module, segs, 0, false).ok()?;
        let types: Vec<Id> = walked
            .entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Symbol(id) if self.is_type(*id) => Some(*id),
                _ => None,
            })
            .collect();
        match types[..] {
            [ty] if self.is_data_type(ty) => Some(ty),
            _ => None,
        }
    }

    /// The struct or enum `method` belongs to: the one type its impl self
    /// type resolves to, as [`Self::place_self`] finds it, when that type is
    /// declared in the method's own target. No method is picked by name.
    pub(crate) fn owner_of(&self, method: &Symbol) -> Option<Id> {
        if method.kind != SymbolKind::Method {
            return None;
        }
        let ty = self
            .owner_type(method)
            .filter(|ty| self.is_data_type(*ty))?;
        let owner = self.symbols.get(&ty)?;
        (self.target_of(owner)? == self.target_of(method)?).then_some(ty)
    }

    /// The target whose module holds `symbol`.
    fn target_of(&self, symbol: &Symbol) -> Option<Id> {
        let module = self.place_of(&symbol.file, &symbol.scope)?;
        self.modules.get(&module).map(|module| module.target)
    }

    /// The root module of the workspace lib `name` names from `module`,
    /// when nothing in `module` binds `name`: no local item, child module,
    /// non-glob `use`, or glob that could supply it.
    fn crate_root(&self, module: Id, name: &str) -> Option<Id> {
        let bound = self.children.contains_key(&(module, name))
            || self.items.contains_key(&(module, name))
            || self.uses.get(&module).is_some_and(|bindings| {
                bindings
                    .iter()
                    .any(|binding| binding.glob || bound_name(binding) == Some(name))
            });
        if bound {
            return None;
        }
        let target = self.modules.get(&module)?.target;
        self.crates.get(&target)?.get(name).copied().flatten()
    }

    fn place_of(&self, file: &str, scope: &str) -> Option<Id> {
        self.places.get(&(file, scope)).copied()
    }

    /// Walks `segs` from `module`. With `method_hops`, a type followed by
    /// one last segment walks to that type's inherent methods.
    fn walk(
        &self,
        module: Id,
        segs: &[String],
        depth: u32,
        method_hops: bool,
    ) -> Result<Walked, Miss> {
        let (first, rest) = segs.split_first().ok_or(Miss::Unresolved)?;
        let mut crossed = false;
        let (mut entities, tier, rest) = match first.as_str() {
            "crate" => {
                let target = self.modules.get(&module).ok_or(Miss::Unresolved)?.target;
                let root = self.roots.get(&target).ok_or(Miss::Unresolved)?;
                (vec![Entity::Module(*root)], Tier::PathExact, rest)
            }
            "self" => (vec![Entity::Module(module)], Tier::PathExact, rest),
            "super" => {
                let mut current = module;
                let mut rest = segs;
                while let [head, tail @ ..] = rest
                    && head == "super"
                {
                    current = self
                        .modules
                        .get(&current)
                        .and_then(|m| m.parent)
                        .ok_or(Miss::Unresolved)?;
                    rest = tail;
                }
                (vec![Entity::Module(current)], Tier::PathExact, rest)
            }
            name if name.is_empty() || STANDARD_CRATES.contains(&name) => {
                return Err(Miss::External);
            }
            name => match self.crate_root(module, name) {
                Some(root) => {
                    crossed = true;
                    (vec![Entity::Module(root)], Tier::CratePath, rest)
                }
                None => {
                    let found = self.lookup(module, name, depth)?;
                    crossed = found.crossed;
                    let tier = if found.local {
                        Tier::Local
                    } else {
                        Tier::UseAlias
                    };
                    (found.entities, tier, rest)
                }
            },
        };
        let mut method_hop = false;
        for (position, seg) in rest.iter().enumerate() {
            let last = position + 1 == rest.len();
            let types: Vec<Entity> = entities
                .iter()
                .copied()
                .filter(|entity| self.is_type_namespace(*entity))
                .collect();
            let [entity] = types[..] else {
                return Err(Miss::Unresolved);
            };
            entities = match entity {
                // Past the first segment the path is inside the workspace,
                // so a miss is unresolved, never external.
                Entity::Module(inner) => {
                    let found = self
                        .lookup(inner, seg, depth)
                        .map_err(|_| Miss::Unresolved)?;
                    crossed |= found.crossed;
                    found.entities
                }
                Entity::Symbol(ty) if last && method_hops && self.is_type(ty) => {
                    method_hop = true;
                    self.inherent
                        .get(seg.as_str())
                        .into_iter()
                        .flatten()
                        .filter(|(_, owner)| *owner == ty)
                        .map(|(method, _)| Entity::Symbol(*method))
                        .collect()
                }
                Entity::Symbol(_) => return Err(Miss::Unresolved),
            };
        }
        Ok(Walked {
            entities,
            tier,
            method_hop,
            crossed,
        })
    }

    /// What `name` binds in `module`.
    fn lookup(&self, module: Id, name: &str, depth: u32) -> Result<Found, Miss> {
        let mut local: Vec<Entity> = self
            .children
            .get(&(module, name))
            .into_iter()
            .flatten()
            .map(|id| Entity::Module(*id))
            .collect();
        let has_module_row = !local.is_empty();
        local.extend(
            self.items
                .get(&(module, name))
                .into_iter()
                .flatten()
                .filter(|id| !(has_module_row && self.kind(**id) == Some(SymbolKind::Module)))
                .map(|id| Entity::Symbol(*id)),
        );
        if !local.is_empty() {
            return Ok(Found {
                entities: local,
                local: true,
                crossed: false,
            });
        }
        if depth >= MAX_DEPTH {
            return Err(Miss::Unresolved);
        }
        let bindings = self.uses.get(&module).map_or(&[][..], Vec::as_slice);
        let mut found = Vec::new();
        let mut crossed = false;
        let mut miss = None;
        for binding in bindings
            .iter()
            .filter(|binding| !binding.glob && bound_name(binding) == Some(name))
        {
            match self.use_entities(binding, depth + 1) {
                Ok(walked) => {
                    crossed |= walked.crossed;
                    found.extend(walked.entities);
                }
                Err(err) => miss = Some(miss.map_or(err, |prior| worse(prior, err))),
            }
        }
        if !found.is_empty() {
            found.sort_by_key(|entity| match entity {
                Entity::Module(id) | Entity::Symbol(id) => *id,
            });
            found.dedup();
            return Ok(Found {
                entities: found,
                local: false,
                crossed,
            });
        }
        if let Some(miss) = miss {
            return Err(miss);
        }
        if bindings.iter().any(|binding| binding.glob) {
            return Err(Miss::Unresolved);
        }
        Err(Miss::External)
    }

    /// What a `use` path names, in any namespace.
    fn use_entities(&self, binding: &UseBinding, depth: u32) -> Result<Walked, Miss> {
        let module = self
            .place_of(&binding.file, &binding.scope)
            .ok_or(Miss::Unresolved)?;
        let segs = segments(&binding.path).ok_or(Miss::Unresolved)?;
        self.walk(module, &segs, depth, false)
    }

    /// The one type an inherent method's owner path names in its module.
    fn owner_type(&self, method: &Symbol) -> Option<Id> {
        let path = method.owner_path.as_deref()?;
        if path.starts_with(['&', '*', '(', '[']) || path.starts_with("dyn") {
            return None;
        }
        let module = self.place_of(&method.file, &method.scope)?;
        let segs = segments(path)?;
        let walked = self.walk(module, &segs, 0, false).ok()?;
        let types: Vec<Id> = walked
            .entities
            .iter()
            .filter_map(|entity| match entity {
                Entity::Symbol(id) if self.is_type(*id) => Some(*id),
                _ => None,
            })
            .collect();
        match types[..] {
            [ty] => Some(ty),
            _ => None,
        }
    }

    fn kind(&self, id: Id) -> Option<SymbolKind> {
        self.symbols.get(&id).map(|symbol| symbol.kind)
    }

    fn is_type(&self, id: Id) -> bool {
        matches!(
            self.kind(id),
            Some(SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait)
        )
    }

    /// A struct or enum: a type a `self` receiver can have.
    fn is_data_type(&self, id: Id) -> bool {
        matches!(self.kind(id), Some(SymbolKind::Struct | SymbolKind::Enum))
    }

    /// Modules and types; a module symbol with no module row is a dead end
    /// that still counts, so a walk through it is unresolved.
    fn is_type_namespace(&self, entity: Entity) -> bool {
        match entity {
            Entity::Module(_) => true,
            Entity::Symbol(id) => self.is_type(id) || self.kind(id) == Some(SymbolKind::Module),
        }
    }

    fn function(&self, entity: Entity) -> Option<Id> {
        match entity {
            Entity::Symbol(id)
                if matches!(
                    self.kind(id),
                    Some(SymbolKind::Function | SymbolKind::Method)
                ) =>
            {
                Some(id)
            }
            _ => None,
        }
    }
}

/// The name a non-glob `use` binds: its alias, or its last segment. `as _`
/// binds nothing.
fn bound_name(binding: &UseBinding) -> Option<&str> {
    match binding.alias.as_deref() {
        Some("_") => None,
        Some(alias) => Some(alias),
        None => Some(last_segment(&binding.path)),
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// The return type in a stored signature, whitespace removed: the text
/// after the first `->` outside brackets, up to a `where` clause. None
/// without one.
fn return_type(signature: &str) -> Option<String> {
    let mut depth = 0_usize;
    let mut chars = signature.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '-' if chars.peek().is_some_and(|(_, next)| *next == '>') => {
                chars.next();
                if depth == 0 {
                    let rest = &signature[at + 2..];
                    let rest = rest.split(" where ").next().unwrap_or(rest);
                    let text: String = rest.split_whitespace().collect();
                    return (!text.is_empty()).then_some(text);
                }
            }
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// `X` for `Result<X, ..>` or `Option<X>`, by the outer type's last path
/// segment.
fn ok_type(returned: &str) -> Option<String> {
    let (outer, inner) = returned.split_once('<')?;
    let inner = inner.strip_suffix('>')?;
    let mut depth = 0_usize;
    let mut arguments = vec![String::new()];
    for ch in inner.chars() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = arguments.last_mut() {
            last.push(ch);
        }
    }
    match (last_segment(outer), &arguments[..]) {
        ("Result", [ok, ..]) | ("Option", [ok]) if !ok.is_empty() => Some(ok.clone()),
        _ => None,
    }
}

/// Unresolved beats external: a binding that names the workspace but
/// cannot be placed is not an external call.
fn worse(a: Miss, b: Miss) -> Miss {
    if a == Miss::Unresolved || b == Miss::Unresolved {
        Miss::Unresolved
    } else {
        Miss::External
    }
}
