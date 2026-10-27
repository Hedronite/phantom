//! Name lookup over the module tree for [`super::resolve`].
//!
//! Inside a module, a local item or child module beats a `use` binding. A
//! name only a glob could supply is not looked up. Inline modules nest by
//! scope, and `super` moves one module up. `self.m()` and `Self::m()` look
//! up the methods of the calling impl's self type (SPEC-w4 G1).

use std::collections::HashMap;

use phantom_fractal_lang::{
    CallRef, Id, Module, Reason, ResolveInput, Symbol, SymbolKind, UseBinding,
};

use super::segments;

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
enum Tier {
    PathExact,
    Local,
    UseAlias,
}

/// A walk's result: the entities the last segment names, how the first
/// segment was bound, and whether the last hop went from a type to one of
/// its methods.
struct Walked {
    entities: Vec<Entity>,
    tier: Tier,
    method_hop: bool,
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
            Tier::PathExact => Reason::PathExact,
            Tier::Local | Tier::UseAlias if segs.len() == 2 && walked.method_hop => Reason::Assoc,
            Tier::Local => Reason::Local,
            Tier::UseAlias => Reason::UseAlias,
        };
        Placed::Edge(target, reason)
    }

    /// Places a call to method `name` of the calling method's own self
    /// type: `self.name()` or `Self::name()` inside `source`.
    ///
    /// `source` must be a method of an `impl` whose self type resolves to
    /// one struct or enum `T`. The candidates are the methods named `name`
    /// whose own impl self type resolves to `T`, inherent or trait impl.
    /// An owner path resolves inside its own target, so every candidate is
    /// in the caller's target. Exactly one candidate gives an edge.
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
        match self.by_type.get(&(ty, name)).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        }
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
            name => {
                let (entities, local) = self.lookup(module, name, depth)?;
                let tier = if local { Tier::Local } else { Tier::UseAlias };
                (entities, tier, rest)
            }
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
                    self.lookup(inner, seg, depth)
                        .map_err(|_| Miss::Unresolved)?
                        .0
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
        })
    }

    /// The entities `name` binds in `module`, and whether they are local.
    fn lookup(&self, module: Id, name: &str, depth: u32) -> Result<(Vec<Entity>, bool), Miss> {
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
            return Ok((local, true));
        }
        if depth >= MAX_DEPTH {
            return Err(Miss::Unresolved);
        }
        let bindings = self.uses.get(&module).map_or(&[][..], Vec::as_slice);
        let mut found = Vec::new();
        let mut miss = None;
        for binding in bindings
            .iter()
            .filter(|binding| !binding.glob && bound_name(binding) == Some(name))
        {
            match self.use_entities(binding, depth + 1) {
                Ok(entities) => found.extend(entities),
                Err(err) => miss = Some(miss.map_or(err, |prior| worse(prior, err))),
            }
        }
        if !found.is_empty() {
            found.sort_by_key(|entity| match entity {
                Entity::Module(id) | Entity::Symbol(id) => *id,
            });
            found.dedup();
            return Ok((found, false));
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
    fn use_entities(&self, binding: &UseBinding, depth: u32) -> Result<Vec<Entity>, Miss> {
        let module = self
            .place_of(&binding.file, &binding.scope)
            .ok_or(Miss::Unresolved)?;
        let segs = segments(&binding.path).ok_or(Miss::Unresolved)?;
        Ok(self.walk(module, &segs, depth, false)?.entities)
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

/// Unresolved beats external: a binding that names the workspace but
/// cannot be placed is not an external call.
fn worse(a: Miss, b: Miss) -> Miss {
    if a == Miss::Unresolved || b == Miss::Unresolved {
        Miss::Unresolved
    } else {
        Miss::External
    }
}
