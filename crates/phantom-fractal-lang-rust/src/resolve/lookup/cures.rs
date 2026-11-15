//! The dogfood cures (SPEC-w13 G2a, D9). Each is tried only for a call
//! the base tiers leave unresolved, so no base edge moves.
//!
//! **(b) A glob import.** The call's first segment is a bare name that
//! nothing in the call's module binds: no item or child module declared
//! there, no non-glob `use` and no shadowing binding. The module's
//! `use p::*` uses whose path walks to a workspace module, by the existing
//! tiers, are the globbed modules. When exactly one item of that name is
//! found across them, as each module's own lookup finds it, the call walks
//! from that module by the existing tiers. The edge's reason is
//! `use-glob`. Two candidates, or none, give no edge.

use phantom_fractal_lang::{CallRef, Id, Node, Reason, SymbolKind};

use super::{Entity, Index, Placed, last_segment, segments};

impl Index<'_> {
    /// What an `import` link to module row `id` names. A later claimant's
    /// row of a shared file, which cure (a) adds, names what the link named
    /// before it was there: the one `mod` item of that name in the parent
    /// module. So no existing link moves (D10).
    pub(crate) fn link_node(&self, id: Id) -> Node {
        if self.canonical(id) == id {
            return Node::Module(id);
        }
        let item = self.modules.get(&id).and_then(|row| {
            let items = self.items.get(&(row.parent?, last_segment(&row.path)))?;
            let [one] = items[..] else {
                return None;
            };
            (self.kind(one) == Some(SymbolKind::Module)).then_some(one)
        });
        item.map_or(Node::Module(id), Node::Symbol)
    }

    /// A `Call`, or a path in a macro's tokens, by the base tiers and,
    /// when they leave it unresolved, by the cures.
    pub(crate) fn place_cured(&self, call: &CallRef, source: Id) -> Placed {
        match self.place(call, source) {
            Placed::Unresolved => self.place_glob(call).unwrap_or(Placed::Unresolved),
            placed => placed,
        }
    }

    /// Cure (b): the call walked from the one globbed module that gives its
    /// first segment.
    fn place_glob(&self, call: &CallRef) -> Option<Placed> {
        if call.shadowed {
            return None;
        }
        let module = self.place_of(&call.file, &call.scope)?;
        let segs = segments(&call.text)?;
        let home = self.glob_home(module, segs.first()?)?;
        let walked = self.walk(home, &segs, 0, true).ok()?;
        let functions: Vec<Id> = walked
            .entities
            .iter()
            .filter_map(|entity| self.function(*entity))
            .collect();
        let [target] = functions[..] else {
            return None;
        };
        Some(Placed::Edge(target, Reason::UseGlob))
    }

    /// The globbed module of `module` that gives `name`, when nothing in
    /// `module` binds `name` and exactly one item of that name is found
    /// across its globbed modules.
    fn glob_home(&self, module: Id, name: &str) -> Option<Id> {
        if !self.declared(module, name).is_empty() || self.bound_by_use(module, name) {
            return None;
        }
        let mut found: Vec<(Entity, Id)> = Vec::new();
        for globbed in self.globbed(module) {
            for entity in self.given(globbed, name) {
                if !found.iter().any(|(seen, _)| *seen == entity) {
                    found.push((entity, globbed));
                }
            }
        }
        let [(_, home)] = found[..] else {
            return None;
        };
        Some(home)
    }

    /// What `name` names in the globbed module `globbed`, as its own
    /// lookup finds it one `use` level down; none when only a glob there
    /// could supply it.
    fn given(&self, globbed: Id, name: &str) -> Vec<Entity> {
        self.lookup(globbed, name, 1)
            .map(|found| found.entities)
            .unwrap_or_default()
    }

    /// True when a non-glob `use` in `module` binds `name`.
    fn bound_by_use(&self, module: Id, name: &str) -> bool {
        self.uses.get(&module).is_some_and(|bindings| {
            bindings
                .iter()
                .any(|binding| !binding.glob && super::bound_name(binding) == Some(name))
        })
    }

    /// The workspace modules the glob uses of `module` reach, each once, in
    /// `use` order.
    fn globbed(&self, module: Id) -> Vec<Id> {
        let mut out: Vec<Id> = Vec::new();
        for binding in self.uses.get(&module).into_iter().flatten() {
            let reached = binding
                .glob
                .then(|| self.use_entities(binding, 1).ok())
                .flatten();
            for entity in reached.into_iter().flat_map(|walked| walked.entities) {
                if let Entity::Module(globbed) = entity
                    && !out.contains(&globbed)
                {
                    out.push(globbed);
                }
            }
        }
        out
    }
}
