//! Tiers 1-3 Go call, import, embed, and interface-satisfaction resolver.

use std::collections::{HashMap, HashSet};

const MAX_EMBED_DEPTH: usize = 16;
const MAX_HINT_DEPTH: u8 = 8;
/// Structural `Impl` edge budget per index (CONTRACT / DoS guard).
const MAX_STRUCTURAL_IMPL_TOTAL: usize = 8_192;

const GO_STDLIB_METHODS: &[&str] = &[
    "Add",
    "And",
    "AppendBinary",
    "AppendText",
    "Clear",
    "Close",
    "Compare",
    "CompareAndSwap",
    "Deadline",
    "Delete",
    "Done",
    "Equal",
    "Err",
    "Error",
    "Flush",
    "Format",
    "GoString",
    "Len",
    "Less",
    "Load",
    "LoadAndDelete",
    "LoadOrStore",
    "Lock",
    "Or",
    "Range",
    "MarshalBinary",
    "MarshalJSON",
    "MarshalText",
    "Next",
    "Peek",
    "Pop",
    "Push",
    "RLock",
    "RUnlock",
    "Read",
    "ReadByte",
    "ReadFrom",
    "Reset",
    "RoundTrip",
    "Scan",
    "Seek",
    "ServeHTTP",
    "Signal",
    "Stat",
    "Store",
    "String",
    "Swap",
    "Text",
    "Unlock",
    "UnmarshalBinary",
    "UnmarshalJSON",
    "UnmarshalText",
    "UnreadByte",
    "Value",
    "Wait",
    "Write",
    "WriteByte",
    "WriteString",
    "WriteTo",
];

use phantom_fractal_lang::{
    CallRef, Edge, Graph, Id, Link, LinkKind, LinkReason, Node, Reason, RefKind, ResolveCounts,
    ResolveInput, Resolver, Symbol, SymbolKind, UseBinding,
};

use crate::extract::default_import_qualifier;

const GO_BUILTINS: &[&str] = &[
    "append",
    "cap",
    "clear",
    "close",
    "complex",
    "copy",
    "delete",
    "imag",
    "len",
    "make",
    "max",
    "min",
    "new",
    "panic",
    "print",
    "println",
    "real",
    "recover",
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
    "comparable",
];

/// Go resolver covering Tiers 1-3 (same-package, per-file imports, typed/embedded/interface methods).
#[derive(Debug, Default, Clone, Copy)]
pub struct GoResolver;

impl GoResolver {
    /// Creates a new Go resolver.
    pub fn new() -> Self {
        Self
    }
}

impl Resolver for GoResolver {
    fn name(&self) -> &'static str {
        "go"
    }

    fn resolve(&self, input: &ResolveInput) -> (Vec<Edge>, ResolveCounts) {
        let graph = self.resolve_graph(input);
        (graph.calls, graph.counts)
    }

    fn resolve_graph(&self, input: &ResolveInput) -> Graph {
        let mut idx = Index::build(input);
        let mut links = Vec::new();
        collect_import_links(input, &idx, &mut links);
        collect_embed_links(input, &mut idx, &mut links);
        collect_impl_links(input, &idx, &mut links);

        let mut out = CallOut::default();
        for call_ref in &input.refs {
            if !matches!(call_ref.kind, RefKind::Embed | RefKind::ValueRef) {
                resolve_one_ref(&idx, call_ref, &mut out);
            }
        }
        Graph {
            calls: out.calls,
            counts: out.counts,
            links,
        }
    }
}

#[derive(Default)]
struct CallOut {
    calls: Vec<Edge>,
    counts: ResolveCounts,
}

impl CallOut {
    fn push(&mut self, ref_id: Id, source: Id, target: Id, confidence: f64, reason: Reason) {
        self.counts.edges += 1;
        self.calls.push(Edge {
            ref_id,
            source,
            target,
            confidence,
            reason,
        });
    }
}

struct Index<'a> {
    modules_by_path: HashMap<&'a str, Id>,
    callable_by_mod: HashMap<(Id, &'a str), Vec<&'a Symbol>>,
    types_by_mod: HashMap<(Id, &'a str), &'a Symbol>,
    types_by_id: HashMap<Id, &'a Symbol>,
    methods_by_owner: HashMap<(Id, &'a str, &'a str), &'a Symbol>,
    owner_methods: HashMap<(Id, &'a str), Vec<&'a Symbol>>,
    unique_methods: HashMap<&'a str, Option<&'a Symbol>>,
    uses_by_file: HashMap<&'a str, Vec<&'a UseBinding>>,
    embeds_adj: HashMap<Id, Vec<Id>>,
}

impl<'a> Index<'a> {
    fn build(input: &'a ResolveInput) -> Self {
        let mut idx = Self {
            modules_by_path: HashMap::new(),
            callable_by_mod: HashMap::new(),
            types_by_mod: HashMap::new(),
            types_by_id: HashMap::new(),
            methods_by_owner: HashMap::new(),
            owner_methods: HashMap::new(),
            unique_methods: HashMap::new(),
            uses_by_file: HashMap::new(),
            embeds_adj: HashMap::new(),
        };
        for m in &input.modules {
            idx.modules_by_path.insert(m.path.as_str(), m.id);
        }
        for u in &input.uses {
            idx.uses_by_file.entry(u.file.as_str()).or_default().push(u);
        }
        for sym in &input.symbols {
            idx.insert_symbol(sym);
        }
        idx.build_unique_methods();
        idx
    }

    fn insert_symbol(&mut self, sym: &'a Symbol) {
        let Some(mod_id) = sym.module else {
            return;
        };
        match sym.kind {
            SymbolKind::Function | SymbolKind::Var => {
                self.callable_by_mod
                    .entry((mod_id, sym.name.as_str()))
                    .or_default()
                    .push(sym);
            }
            SymbolKind::Struct | SymbolKind::Interface | SymbolKind::TypeAlias => {
                self.types_by_mod.insert((mod_id, sym.name.as_str()), sym);
                self.types_by_id.insert(sym.id, sym);
            }
            SymbolKind::Method => self.insert_method_symbol(mod_id, sym),
            _ => {}
        }
    }

    fn build_unique_methods(&mut self) {
        for (&(mod_id, owner), ms) in &self.owner_methods {
            let is_iface = self
                .types_by_mod
                .get(&(mod_id, owner))
                .is_some_and(|ty| ty.kind == SymbolKind::Interface);
            if is_iface {
                continue;
            }
            for &m in ms {
                self.unique_methods
                    .entry(m.name.as_str())
                    .and_modify(|slot| *slot = None)
                    .or_insert(Some(m));
            }
        }
    }

    fn insert_method_symbol(&mut self, mod_id: Id, sym: &'a Symbol) {
        let Some(owner) = sym.owner.as_deref() else {
            return;
        };
        self.methods_by_owner
            .insert((mod_id, owner, sym.name.as_str()), sym);
        self.owner_methods
            .entry((mod_id, owner))
            .or_default()
            .push(sym);
    }
}

fn collect_import_links(input: &ResolveInput, idx: &Index<'_>, links: &mut Vec<Link>) {
    for u in &input.uses {
        if u.alias.as_deref() == Some("_") {
            continue;
        }
        let (Some(src_mod), Some(&dst_mod)) = (u.module, idx.modules_by_path.get(u.path.as_str()))
        else {
            continue;
        };
        let reason = if u.alias.is_some() {
            LinkReason::UseAlias
        } else {
            LinkReason::PathExact
        };
        links.push(Link {
            kind: LinkKind::Import,
            source: Node::Module(src_mod),
            target: Node::Module(dst_mod),
            confidence: 1.0,
            reason,
            ref_id: Some(u.id),
        });
    }
}

fn collect_embed_links(input: &ResolveInput, idx: &mut Index<'_>, links: &mut Vec<Link>) {
    for r in &input.refs {
        if r.kind != RefKind::Embed || r.receiver_type.as_deref() == Some("assert_impl") {
            continue;
        }
        let (Some(mod_id), Some(owner_name)) = (r.module, r.receiver.as_deref()) else {
            continue;
        };
        let Some(&outer) = idx.types_by_mod.get(&(mod_id, owner_name)) else {
            continue;
        };
        let ResolvedType::Workspace(inner) = resolve_type_name(idx, &r.file, Some(mod_id), &r.text)
        else {
            continue;
        };
        idx.embeds_adj.entry(outer.id).or_default().push(inner.id);
        links.push(Link {
            kind: LinkKind::Embeds,
            source: Node::Symbol(outer.id),
            target: Node::Symbol(inner.id),
            confidence: 1.0,
            reason: LinkReason::EmbeddedType,
            ref_id: Some(r.id),
        });
    }
}

fn collect_impl_links(input: &ResolveInput, idx: &Index<'_>, links: &mut Vec<Link>) {
    let explicit = collect_explicit_impls(input, idx, links);
    let mut sets: HashMap<Id, HashSet<&str>> = HashMap::new();
    let mut by_method: HashMap<&str, Vec<Id>> = HashMap::new();
    for st in input
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Struct)
    {
        let mset = method_set_memo(idx, st.id, &sets);
        for &name in &mset {
            by_method.entry(name).or_default().push(st.id);
        }
        sets.insert(st.id, mset);
    }
    let mut structural_left = MAX_STRUCTURAL_IMPL_TOTAL;
    for iface in input
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Interface)
    {
        if structural_left == 0 {
            break;
        }
        let req = method_set_memo(idx, iface.id, &sets);
        if !req.is_empty() {
            structural_left = match_structural_impls(
                iface.id,
                &req,
                &sets,
                &by_method,
                &explicit,
                links,
                structural_left,
            );
        }
    }
}

fn collect_explicit_impls(
    input: &ResolveInput,
    idx: &Index<'_>,
    links: &mut Vec<Link>,
) -> HashSet<(Id, Id)> {
    let mut explicit = HashSet::new();
    for r in &input.refs {
        let Some((st_id, iface_id)) = explicit_assert_pair(idx, r) else {
            continue;
        };
        if explicit.insert((st_id, iface_id)) {
            links.push(Link {
                kind: LinkKind::Impl,
                source: Node::Symbol(st_id),
                target: Node::Symbol(iface_id),
                confidence: 1.0,
                reason: LinkReason::ImplTrait,
                ref_id: None,
            });
        }
    }
    explicit
}

fn explicit_assert_pair(idx: &Index<'_>, r: &CallRef) -> Option<(Id, Id)> {
    if r.kind != RefKind::Embed || r.receiver_type.as_deref() != Some("assert_impl") {
        return None;
    }
    let concrete_name = r.receiver.as_deref()?;
    let (ResolvedType::Workspace(st), ResolvedType::Workspace(iface)) = (
        resolve_type_name(idx, &r.file, r.module, concrete_name),
        resolve_type_name(idx, &r.file, r.module, &r.text),
    ) else {
        return None;
    };
    (st.kind != SymbolKind::Interface && iface.kind == SymbolKind::Interface)
        .then_some((st.id, iface.id))
}

#[allow(clippy::too_many_arguments)]
fn match_structural_impls(
    iface_id: Id,
    req: &HashSet<&str>,
    sets: &HashMap<Id, HashSet<&str>>,
    by_method: &HashMap<&str, Vec<Id>>,
    explicit: &HashSet<(Id, Id)>,
    links: &mut Vec<Link>,
    mut budget: usize,
) -> usize {
    let Some(rarest) = req
        .iter()
        .min_by_key(|m| by_method.get(*m).map_or(0, Vec::len))
        .and_then(|m| by_method.get(m))
    else {
        return budget;
    };
    for &st_id in rarest {
        if budget == 0 {
            break;
        }
        if explicit.contains(&(st_id, iface_id)) {
            continue;
        }
        if sets
            .get(&st_id)
            .is_some_and(|avail| req.iter().all(|m| avail.contains(m)))
        {
            links.push(Link {
                kind: LinkKind::Impl,
                source: Node::Symbol(st_id),
                target: Node::Symbol(iface_id),
                confidence: 0.85,
                reason: LinkReason::StructuralImpl,
                ref_id: None,
            });
            budget -= 1;
        }
    }
    budget
}

fn method_set_memo<'a>(
    idx: &Index<'a>,
    root_id: Id,
    cache: &HashMap<Id, HashSet<&'a str>>,
) -> HashSet<&'a str> {
    let mut names = HashSet::new();
    let mut visited = HashSet::new();
    let mut frontier = vec![root_id];
    for _depth in 0..=MAX_EMBED_DEPTH {
        frontier.retain(|id| visited.insert(*id));
        if frontier.is_empty() {
            break;
        }
        frontier = step_method_set(idx, &frontier, cache, &mut names);
    }
    names
}

fn step_method_set<'a>(
    idx: &Index<'a>,
    frontier: &[Id],
    cache: &HashMap<Id, HashSet<&'a str>>,
    names: &mut HashSet<&'a str>,
) -> Vec<Id> {
    let mut next = Vec::new();
    for &id in frontier {
        if let Some(cached) = cache.get(&id) {
            names.extend(cached.iter().copied());
            continue;
        }
        push_type_methods(id, idx, names);
        if let Some(embeds) = idx.embeds_adj.get(&id) {
            next.extend(embeds.iter().copied());
        }
    }
    next
}

fn push_type_methods<'a>(id: Id, idx: &Index<'a>, names: &mut HashSet<&'a str>) {
    let Some(cur) = idx.types_by_id.get(&id).copied() else {
        return;
    };
    let Some(ms) = cur
        .module
        .and_then(|mid| idx.owner_methods.get(&(mid, cur.name.as_str())))
    else {
        return;
    };
    for m in ms {
        names.insert(m.name.as_str());
    }
}

fn resolve_one_ref(idx: &Index<'_>, call_ref: &CallRef, out: &mut CallOut) {
    let Some(source) = call_ref.from else {
        out.counts.no_source += 1;
        return;
    };
    if call_ref.shadowed {
        return;
    }
    if call_ref.kind == RefKind::Call {
        resolve_func_call(idx, call_ref, source, out);
    } else if call_ref.kind == RefKind::MethodCall {
        resolve_method_call(idx, call_ref, source, out);
    }
}

fn resolve_func_call(idx: &Index<'_>, call_ref: &CallRef, source: Id, out: &mut CallOut) {
    let require_generic = call_ref.receiver_type.as_deref() == Some("type_inst");
    if let Some((qual, name)) = call_ref.text.split_once('.') {
        resolve_qualified_call(idx, call_ref, source, (qual, name), require_generic, out);
        return;
    }
    resolve_bare_call(idx, call_ref, source, require_generic, out);
}

fn resolve_qualified_call(
    idx: &Index<'_>,
    call_ref: &CallRef,
    source: Id,
    (qual, name): (&str, &str),
    require_generic: bool,
    out: &mut CallOut,
) {
    match resolve_imported_callable(idx, &call_ref.file, qual, name) {
        Lookup::Workspace(sym) if is_valid_callee(sym, require_generic) => {
            out.push(call_ref.id, source, sym.id, 1.0, Reason::PackageImport);
        }
        Lookup::Workspace(_) | Lookup::Missing => out.counts.unresolved += 1,
        Lookup::External => out.counts.external += 1,
    }
}

fn resolve_bare_call(
    idx: &Index<'_>,
    call_ref: &CallRef,
    source: Id,
    require_generic: bool,
    out: &mut CallOut,
) {
    if let Some(sym) = unique_local_callable(idx, call_ref.module, &call_ref.text)
        && is_valid_callee(sym, require_generic)
    {
        out.push(call_ref.id, source, sym.id, 1.0, Reason::Local);
        return;
    }
    resolve_dot_or_builtin(idx, call_ref, source, require_generic, out);
}

fn unique_local_callable<'a>(
    idx: &Index<'a>,
    mod_id: Option<Id>,
    name: &str,
) -> Option<&'a Symbol> {
    let cands = idx.callable_by_mod.get(&(mod_id?, name))?;
    (cands.len() == 1).then_some(cands[0])
}

fn resolve_dot_or_builtin(
    idx: &Index<'_>,
    call_ref: &CallRef,
    source: Id,
    require_generic: bool,
    out: &mut CallOut,
) {
    match resolve_dot_import_callable(idx, &call_ref.file, &call_ref.text) {
        Lookup::Workspace(sym) if is_valid_callee(sym, require_generic) => {
            out.push(call_ref.id, source, sym.id, 0.95, Reason::PackageImport);
        }
        Lookup::External => out.counts.external += 1,
        Lookup::Workspace(_) | Lookup::Missing => {
            count_missing_or_builtin(&call_ref.text, require_generic, out);
        }
    }
}

fn count_missing_or_builtin(name: &str, require_generic: bool, out: &mut CallOut) {
    if !require_generic && GO_BUILTINS.contains(&name) {
        out.counts.external += 1;
    } else {
        out.counts.unresolved += 1;
    }
}

fn is_valid_callee(sym: &Symbol, require_generic: bool) -> bool {
    !require_generic
        || (sym.kind == SymbolKind::Function && has_type_parameters(sym.signature.as_deref()))
}

fn has_type_parameters(sig: Option<&str>) -> bool {
    let Some(after) = sig.and_then(skip_signature_header) else {
        return false;
    };
    matches!((after.find('['), after.find('(')), (Some(b), Some(p)) if b < p)
}

fn skip_signature_header(sig: &str) -> Option<&str> {
    let mut after = sig.strip_prefix("func")?.trim_start();
    if after.starts_with('(') {
        after = skip_balanced(after, '(', ')')?.trim_start();
    }
    Some(after)
}

fn resolve_method_call(idx: &Index<'_>, call_ref: &CallRef, source: Id, out: &mut CallOut) {
    match resolve_recv_type(idx, call_ref) {
        ResolvedType::Workspace(ty) => resolve_workspace_method(idx, call_ref, source, ty, out),
        ResolvedType::External => out.counts.external += 1,
        ResolvedType::Unknown => resolve_unknown_method(idx, call_ref, source, out),
    }
}

fn resolve_unknown_method(idx: &Index<'_>, call_ref: &CallRef, source: Id, out: &mut CallOut) {
    if !GO_STDLIB_METHODS.contains(&call_ref.text.as_str())
        && let Some(Some(target)) = idx.unique_methods.get(call_ref.text.as_str()).copied()
    {
        out.push(call_ref.id, source, target.id, 0.70, Reason::UniqueMethod);
        return;
    }
    mark_unresolved_method(out);
}

fn resolve_recv_type<'a>(idx: &Index<'a>, call_ref: &CallRef) -> ResolvedType<'a> {
    call_ref
        .receiver_type
        .as_deref()
        .map_or(ResolvedType::Unknown, |hint| {
            resolve_hint_type(idx, call_ref, hint)
        })
}

fn resolve_workspace_method(
    idx: &Index<'_>,
    call_ref: &CallRef,
    source: Id,
    ty: &Symbol,
    out: &mut CallOut,
) {
    if ty.kind == SymbolKind::Interface {
        if let Some(target) = find_method_bfs(idx, ty, &call_ref.text, true) {
            out.push(call_ref.id, source, target.id, 1.0, Reason::InterfaceMethod);
        } else {
            mark_unresolved_method(out);
        }
        return;
    }
    if let Some(target) = direct_method(idx, ty, &call_ref.text) {
        out.push(call_ref.id, source, target.id, 1.0, Reason::TypedMethod);
    } else if let Some(target) = find_method_bfs(idx, ty, &call_ref.text, false) {
        out.push(call_ref.id, source, target.id, 0.95, Reason::EmbeddedMethod);
    } else {
        mark_unresolved_method(out);
    }
}

fn direct_method<'a>(idx: &Index<'a>, ty: &Symbol, method: &str) -> Option<&'a Symbol> {
    let mod_id = ty.module?;
    idx.methods_by_owner
        .get(&(mod_id, ty.name.as_str(), method))
        .copied()
}

fn mark_unresolved_method(out: &mut CallOut) {
    out.counts.unresolved += 1;
    out.counts.method_calls += 1;
}

fn find_method_bfs<'a>(
    idx: &Index<'a>,
    root_ty: &'a Symbol,
    method: &str,
    include_root: bool,
) -> Option<&'a Symbol> {
    let mut visited = HashSet::new();
    if !include_root {
        visited.insert(root_ty.id);
    }
    let mut frontier = initial_bfs_frontier(idx, root_ty.id, include_root);
    for _depth in 0..=MAX_EMBED_DEPTH {
        frontier.retain(|id| visited.insert(*id));
        if frontier.is_empty() {
            return None;
        }
        let (found, next) = step_method_bfs(idx, &idx.types_by_id, &frontier, method);
        if found.len() == 1 {
            return Some(found[0]);
        }
        if !found.is_empty() {
            return None;
        }
        frontier = next;
    }
    None
}

fn initial_bfs_frontier(idx: &Index<'_>, root_id: Id, include_root: bool) -> Vec<Id> {
    if include_root {
        vec![root_id]
    } else {
        idx.embeds_adj.get(&root_id).cloned().unwrap_or_default()
    }
}

fn step_method_bfs<'a>(
    idx: &Index<'a>,
    all_types: &HashMap<Id, &'a Symbol>,
    frontier: &[Id],
    method: &str,
) -> (Vec<&'a Symbol>, Vec<Id>) {
    let mut found = Vec::new();
    let mut next = Vec::new();
    for &id in frontier {
        if let Some(m) = all_types
            .get(&id)
            .copied()
            .and_then(|ty| direct_method(idx, ty, method))
        {
            found.push(m);
        }
        if let Some(embeds) = idx.embeds_adj.get(&id) {
            next.extend(embeds.iter().copied());
        }
    }
    (found, next)
}

enum Lookup<'a> {
    Workspace(&'a Symbol),
    External,
    Missing,
}

enum ImportTarget {
    Workspace(Id),
    External,
    NotImported,
}

fn find_import_by_qualifier(idx: &Index<'_>, file: &str, qual: &str) -> ImportTarget {
    let Some(uses) = idx.uses_by_file.get(file) else {
        return ImportTarget::NotImported;
    };
    uses.iter()
        .find_map(|u| match_import_binding(idx, u, qual))
        .unwrap_or(ImportTarget::NotImported)
}

fn match_import_binding(idx: &Index<'_>, u: &UseBinding, qual: &str) -> Option<ImportTarget> {
    if u.glob || u.alias.as_deref() == Some("_") {
        return None;
    }
    let u_qual = u
        .alias
        .as_deref()
        .unwrap_or_else(|| default_import_qualifier(&u.path));
    if u_qual != qual {
        return None;
    }
    Some(match idx.modules_by_path.get(u.path.as_str()) {
        Some(&mod_id) => ImportTarget::Workspace(mod_id),
        None => ImportTarget::External,
    })
}

fn resolve_imported_callable<'a>(
    idx: &Index<'a>,
    file: &str,
    qual: &str,
    name: &str,
) -> Lookup<'a> {
    match find_import_by_qualifier(idx, file, qual) {
        ImportTarget::Workspace(mod_id) => unique_local_callable(idx, Some(mod_id), name)
            .map_or(Lookup::Missing, Lookup::Workspace),
        ImportTarget::External => Lookup::External,
        ImportTarget::NotImported => Lookup::Missing,
    }
}

fn resolve_dot_import_callable<'a>(idx: &Index<'a>, file: &str, name: &str) -> Lookup<'a> {
    let Some(uses) = idx.uses_by_file.get(file) else {
        return Lookup::Missing;
    };
    let mut matches = Vec::new();
    let mut any_external = false;
    for u in uses.iter().copied().filter(|u| u.glob) {
        match idx.modules_by_path.get(u.path.as_str()) {
            Some(&mod_id) => extend_callables(idx, mod_id, name, &mut matches),
            None => any_external = true,
        }
    }
    pick_dot_match(matches, any_external)
}

fn pick_dot_match<'a>(matches: Vec<&'a Symbol>, any_external: bool) -> Lookup<'a> {
    if matches.len() == 1 {
        Lookup::Workspace(matches[0])
    } else if any_external && matches.is_empty() {
        Lookup::External
    } else {
        Lookup::Missing
    }
}

fn extend_callables<'a>(idx: &Index<'a>, mod_id: Id, name: &str, matches: &mut Vec<&'a Symbol>) {
    if let Some(cands) = idx.callable_by_mod.get(&(mod_id, name)) {
        matches.extend(cands.iter().copied());
    }
}

enum ResolvedType<'a> {
    Workspace(&'a Symbol),
    External,
    Unknown,
}

fn resolve_hint_type<'a>(idx: &Index<'a>, call_ref: &CallRef, hint: &str) -> ResolvedType<'a> {
    resolve_hint_type_d(idx, call_ref, hint, 0)
}

fn resolve_hint_type_d<'a>(
    idx: &Index<'a>,
    call_ref: &CallRef,
    hint: &str,
    depth: u8,
) -> ResolvedType<'a> {
    if depth >= MAX_HINT_DEPTH {
        return ResolvedType::Unknown;
    }
    if let Some(ty_name) = hint.strip_prefix("type:") {
        return resolve_type_name(idx, &call_ref.file, call_ref.module, ty_name);
    }
    if let Some(mret) = hint.strip_prefix("mret:") {
        return resolve_method_return_type(idx, call_ref, mret, depth + 1);
    }
    if let Some(field) = hint.strip_prefix("field:") {
        return resolve_embedded_field_type(idx, call_ref, field, depth + 1);
    }
    resolve_callee_hint_type(idx, call_ref, hint)
}

fn resolve_callee_hint_type<'a>(
    idx: &Index<'a>,
    call_ref: &CallRef,
    hint: &str,
) -> ResolvedType<'a> {
    let (require_generic, rest) = match hint.strip_prefix("inst_") {
        Some(r) => (true, r),
        None => (false, hint),
    };
    strip_ret_prefix(rest).map_or(ResolvedType::Unknown, |callee_name| {
        resolve_callee_return_type(idx, call_ref, callee_name, require_generic)
    })
}

fn resolve_method_return_type<'a>(
    idx: &Index<'a>,
    call_ref: &CallRef,
    mret: &str,
    depth: u8,
) -> ResolvedType<'a> {
    let Some((recv_hint, method_name)) = mret.rsplit_once('|') else {
        return ResolvedType::Unknown;
    };
    let ResolvedType::Workspace(owner_ty) = resolve_hint_type_d(idx, call_ref, recv_hint, depth)
    else {
        return ResolvedType::Unknown;
    };
    let Some(m_sym) = direct_method(idx, owner_ty, method_name)
        .or_else(|| find_method_bfs(idx, owner_ty, method_name, false))
    else {
        return ResolvedType::Unknown;
    };
    let Some(ret_type) = m_sym.signature.as_deref().and_then(parse_first_return_type) else {
        return ResolvedType::Unknown;
    };
    resolve_type_name(idx, &m_sym.file, m_sym.module, &ret_type)
}

fn resolve_embedded_field_type<'a>(
    idx: &Index<'a>,
    call_ref: &CallRef,
    spec: &str,
    depth: u8,
) -> ResolvedType<'a> {
    let Some((owner_hint, field_name)) = spec.rsplit_once('|') else {
        return ResolvedType::Unknown;
    };
    let ResolvedType::Workspace(owner_ty) = resolve_hint_type_d(idx, call_ref, owner_hint, depth)
    else {
        return ResolvedType::Unknown;
    };
    find_embedded_field_bfs(idx, owner_ty.id, field_name)
        .map_or(ResolvedType::Unknown, ResolvedType::Workspace)
}

enum FieldStep<'a> {
    Found(&'a Symbol),
    Ambiguous,
    Next(Vec<Id>),
}

fn find_embedded_field_bfs<'a>(
    idx: &Index<'a>,
    root_id: Id,
    field_name: &str,
) -> Option<&'a Symbol> {
    let mut visited = HashSet::from([root_id]);
    let mut frontier = idx.embeds_adj.get(&root_id).cloned().unwrap_or_default();
    for _depth in 0..MAX_EMBED_DEPTH {
        frontier.retain(|id| visited.insert(*id));
        match step_embedded_field(idx, &frontier, field_name) {
            FieldStep::Found(sym) => return Some(sym),
            FieldStep::Ambiguous => return None,
            FieldStep::Next(next) => frontier = next,
        }
    }
    None
}

fn step_embedded_field<'a>(idx: &Index<'a>, frontier: &[Id], field_name: &str) -> FieldStep<'a> {
    let found: Vec<&Symbol> = frontier
        .iter()
        .filter_map(|id| idx.types_by_id.get(id).copied())
        .filter(|sym| sym.name == field_name)
        .collect();
    if found.len() == 1 {
        return FieldStep::Found(found[0]);
    }
    if !found.is_empty() {
        return FieldStep::Ambiguous;
    }
    let next = frontier
        .iter()
        .filter_map(|id| idx.embeds_adj.get(id))
        .flatten()
        .copied()
        .collect();
    FieldStep::Next(next)
}

fn resolve_callee_return_type<'a>(
    idx: &Index<'a>,
    call_ref: &CallRef,
    callee_name: &str,
    require_generic: bool,
) -> ResolvedType<'a> {
    let fn_sym = match lookup_callee_sym(idx, call_ref, callee_name) {
        Lookup::Workspace(s) if is_valid_callee(s, require_generic) => s,
        Lookup::External => return ResolvedType::External,
        Lookup::Workspace(_) | Lookup::Missing => return ResolvedType::Unknown,
    };
    let Some(ret_type) = fn_sym
        .signature
        .as_deref()
        .and_then(parse_first_return_type)
    else {
        return ResolvedType::Unknown;
    };
    resolve_type_name(idx, &fn_sym.file, fn_sym.module, &ret_type)
}

fn lookup_callee_sym<'a>(idx: &Index<'a>, call_ref: &CallRef, callee_name: &str) -> Lookup<'a> {
    if let Some((qual, name)) = callee_name.split_once('.') {
        return resolve_imported_callable(idx, &call_ref.file, qual, name);
    }
    unique_local_callable(idx, call_ref.module, callee_name)
        .map_or(Lookup::Missing, Lookup::Workspace)
}

fn strip_ret_prefix(hint: &str) -> Option<&str> {
    hint.strip_prefix("ret:")
        .or_else(|| hint.strip_prefix("ret?:"))
        .or_else(|| hint.strip_prefix("ctor:"))
        .or_else(|| hint.strip_prefix("ctor?:"))
}

fn resolve_type_name<'a>(
    idx: &Index<'a>,
    file: &str,
    mod_id: Option<Id>,
    ty_name: &str,
) -> ResolvedType<'a> {
    if let Some((qual, name)) = ty_name.split_once('.') {
        return resolve_qualified_type(idx, file, qual, name);
    }
    if let Some(sym) = mod_id
        .and_then(|mid| idx.types_by_mod.get(&(mid, ty_name)))
        .copied()
    {
        return ResolvedType::Workspace(sym);
    }
    resolve_dot_import_type(idx, file, ty_name)
}

fn resolve_qualified_type<'a>(
    idx: &Index<'a>,
    file: &str,
    qual: &str,
    name: &str,
) -> ResolvedType<'a> {
    match find_import_by_qualifier(idx, file, qual) {
        ImportTarget::Workspace(dst_mod) => idx
            .types_by_mod
            .get(&(dst_mod, name))
            .map_or(ResolvedType::Unknown, |s| ResolvedType::Workspace(s)),
        ImportTarget::External => ResolvedType::External,
        ImportTarget::NotImported => ResolvedType::Unknown,
    }
}

fn resolve_dot_import_type<'a>(idx: &Index<'a>, file: &str, ty_name: &str) -> ResolvedType<'a> {
    let Some(uses) = idx.uses_by_file.get(file) else {
        return ResolvedType::Unknown;
    };
    let mut found = Vec::new();
    for u in uses.iter().copied().filter(|u| u.glob) {
        if let Some(&mid) = idx.modules_by_path.get(u.path.as_str())
            && let Some(&sym) = idx.types_by_mod.get(&(mid, ty_name))
        {
            found.push(sym);
        }
    }
    if found.len() == 1 {
        ResolvedType::Workspace(found[0])
    } else {
        ResolvedType::Unknown
    }
}

fn parse_first_return_type(sig: &str) -> Option<String> {
    let after = skip_signature_header(sig)?;
    let idx = after.find('(')?;
    let ret = skip_balanced(&after[idx..], '(', ')')?.trim();
    let first = (!ret.is_empty())
        .then(|| unwrap_first_result(ret))
        .flatten()?;
    let clean = crate::extract::bare_type_name(first);
    (!clean.is_empty()).then_some(clean)
}

fn unwrap_first_result(ret: &str) -> Option<&str> {
    if let Some(inner) = ret.strip_prefix('(') {
        let end = inner.rfind(')')?;
        return Some(first_comma_item(&inner[..end]));
    }
    Some(ret)
}

fn skip_balanced(s: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(&s[i + ch.len_utf8()..]);
            }
        }
    }
    None
}

fn first_comma_item(s: &str) -> &str {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        depth = step_bracket_depth(depth, ch);
        if ch == ',' && depth == 0 {
            return s[..i].trim();
        }
    }
    s.trim()
}

fn step_bracket_depth(depth: usize, ch: char) -> usize {
    match ch {
        '(' | '[' => depth + 1,
        ')' | ']' => depth.saturating_sub(1),
        _ => depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_return_types_including_methods_and_nested_tuples() {
        assert_eq!(
            first_comma_item("Map[int, string], error"),
            "Map[int, string]"
        );
        assert_eq!(
            parse_first_return_type("func (s *S) Get() (func(int) int, error)"),
            Some("func(int)int".to_owned())
        );
        assert_eq!(
            parse_first_return_type("func Pair(cb func(int, int)) (Map[int, string], error)"),
            Some("Map".to_owned())
        );
        assert_eq!(
            parse_first_return_type("func SingleParen() (*Box)"),
            Some("Box".to_owned())
        );
        assert_eq!(parse_first_return_type("func Void()"), None);
        assert_eq!(parse_first_return_type("func Broken("), None);
        assert!(has_type_parameters(Some("func (s *S) M[T any]()")));
        assert!(!has_type_parameters(Some("not_a_func")));
        assert!(!has_type_parameters(Some("func (broken")));

        let inp = ResolveInput {
            targets: vec![],
            modules: vec![phantom_fractal_lang::Module {
                id: 1,
                target: 1,
                parent: None,
                path: "example.com/p1".to_owned(),
                file: String::new(),
                scope: String::new(),
            }],
            symbols: vec![Symbol {
                id: 10,
                kind: SymbolKind::Function,
                name: "F".to_owned(),
                owner: None,
                owner_path: None,
                trait_name: None,
                scope: String::new(),
                module: Some(1),
                file: "p1/p1.go".to_owned(),
                signature: Some("func F()".to_owned()),
            }],
            uses: vec![
                UseBinding {
                    id: 1,
                    module: Some(2),
                    scope: String::new(),
                    file: "c.go".to_owned(),
                    path: "example.com/p1".to_owned(),
                    alias: None,
                    glob: true,
                },
                UseBinding {
                    id: 2,
                    module: Some(2),
                    scope: String::new(),
                    file: "c.go".to_owned(),
                    path: "example.com/p1".to_owned(),
                    alias: Some("_".to_owned()),
                    glob: false,
                },
            ],
            refs: vec![
                CallRef {
                    id: 3,
                    from: Some(20),
                    module: Some(2),
                    scope: String::new(),
                    file: "c.go".to_owned(),
                    kind: RefKind::Call,
                    text: "p1.F".to_owned(),
                    receiver: None,
                    receiver_type: None,
                    shadowed: false,
                },
                CallRef {
                    id: 4,
                    from: Some(20),
                    module: Some(2),
                    scope: String::new(),
                    file: "c.go".to_owned(),
                    kind: RefKind::Call,
                    text: "_.F".to_owned(),
                    receiver: None,
                    receiver_type: None,
                    shadowed: false,
                },
            ],
            deps: vec![],
        };
        let (edges, counts) = GoResolver::new().resolve(&inp);
        assert!(edges.is_empty());
        assert_eq!(counts.unresolved, 2);

        // Cover hint depth cap, explicit_assert_pair guards, interface vs struct method return lookup.
        let dummy_ref = &inp.refs[0];
        assert!(matches!(
            resolve_hint_type_d(&idx_probe(), dummy_ref, "type:X", MAX_HINT_DEPTH),
            ResolvedType::Unknown
        ));
        assert!(matches!(
            resolve_hint_type_d(&idx_probe(), dummy_ref, "type:X", MAX_HINT_DEPTH - 1),
            ResolvedType::Unknown
        ));
        assert!(matches!(
            resolve_embedded_field_type(&idx_probe(), dummy_ref, "bad", 0),
            ResolvedType::Unknown
        ));
        assert!(matches!(
            resolve_method_return_type(&idx_probe(), dummy_ref, "bad", 0),
            ResolvedType::Unknown
        ));

        let mut mod_inp = inp.clone();
        mod_inp.symbols.extend([
            Symbol {
                id: 30,
                kind: SymbolKind::Interface,
                name: "IfaceA".to_owned(),
                owner: None,
                owner_path: None,
                trait_name: None,
                scope: String::new(),
                module: Some(1),
                file: "p1/p1.go".to_owned(),
                signature: None,
            },
            Symbol {
                id: 31,
                kind: SymbolKind::Interface,
                name: "IfaceB".to_owned(),
                owner: None,
                owner_path: None,
                trait_name: None,
                scope: String::new(),
                module: Some(1),
                file: "p1/p1.go".to_owned(),
                signature: None,
            },
            Symbol {
                id: 32,
                kind: SymbolKind::Struct,
                name: "StructA".to_owned(),
                owner: None,
                owner_path: None,
                trait_name: None,
                scope: String::new(),
                module: Some(1),
                file: "p1/p1.go".to_owned(),
                signature: None,
            },
            Symbol {
                id: 33,
                kind: SymbolKind::Method,
                name: "MakeStruct".to_owned(),
                owner: Some("IfaceA".to_owned()),
                owner_path: Some("IfaceA".to_owned()),
                trait_name: None,
                scope: String::new(),
                module: Some(1),
                file: "p1/p1.go".to_owned(),
                signature: Some("func MakeStruct() StructA".to_owned()),
            },
        ]);
        mod_inp.refs.extend([
            CallRef {
                id: 40,
                from: None,
                module: Some(1),
                scope: String::new(),
                file: "p1/p1.go".to_owned(),
                kind: RefKind::Embed,
                text: "IfaceB".to_owned(),
                receiver: Some("IfaceA".to_owned()),
                receiver_type: Some("assert_impl".to_owned()),
                shadowed: false,
            },
            CallRef {
                id: 41,
                from: None,
                module: Some(1),
                scope: String::new(),
                file: "p1/p1.go".to_owned(),
                kind: RefKind::Embed,
                text: "StructA".to_owned(),
                receiver: Some("StructA".to_owned()),
                receiver_type: Some("assert_impl".to_owned()),
                shadowed: false,
            },
            CallRef {
                id: 42,
                from: None,
                module: Some(1),
                scope: String::new(),
                file: "p1/p1.go".to_owned(),
                kind: RefKind::Call,
                text: "IfaceB".to_owned(),
                receiver: Some("StructA".to_owned()),
                receiver_type: Some("assert_impl".to_owned()),
                shadowed: false,
            },
        ]);
        let g2 = GoResolver::new().resolve_graph(&mod_inp);
        assert!(
            !g2.links
                .iter()
                .any(|l| l.kind == LinkKind::Impl || l.kind == LinkKind::Embeds)
        );
        let idx2 = Index::build(&mod_inp);
        let iref = CallRef {
            id: 43,
            from: Some(20),
            module: Some(1),
            scope: String::new(),
            file: "p1/p1.go".to_owned(),
            kind: RefKind::MethodCall,
            text: "X".to_owned(),
            receiver: Some("i".to_owned()),
            receiver_type: None,
            shadowed: false,
        };
        assert!(matches!(
            resolve_method_return_type(&idx2, &iref, "type:IfaceA|MakeStruct", 0),
            ResolvedType::Workspace(s) if s.id == 32
        ));
        let mut h_mret_pass = "type:IfaceA".to_owned();
        for _ in 0..7 {
            // 7 hops stay under MAX_HINT_DEPTH = 8 (Wait: use recursive MakeSelf or Chain)
            h_mret_pass = format!("mret:{h_mret_pass}|MakeStruct");
        }
        let _ = h_mret_pass;
        // Real 8-hop self-referential method return and field embed chains hit MAX_HINT_DEPTH at depth=8:
        let mut cyc_inp = mod_inp.clone();
        cyc_inp.symbols.push(Symbol {
            id: 34,
            kind: SymbolKind::Method,
            name: "NextSelf".to_owned(),
            owner: Some("IfaceA".to_owned()),
            owner_path: Some("IfaceA".to_owned()),
            trait_name: None,
            scope: String::new(),
            module: Some(1),
            file: "p1/p1.go".to_owned(),
            signature: Some("func NextSelf() IfaceA".to_owned()),
        });
        let idx3 = Index::build(&cyc_inp);
        let mut mret8 = "type:IfaceA".to_owned();
        let mut fld8 = "type:IfaceA".to_owned();
        for i in 0..8 {
            mret8 = format!("mret:{mret8}|NextSelf");
            let next_name = if i % 2 == 0 { "IfaceB" } else { "IfaceA" };
            fld8 = format!("field:{fld8}|{next_name}");
        }
        assert!(matches!(
            resolve_hint_type_d(&idx3, &iref, &mret8, 0),
            ResolvedType::Unknown
        ));
        let mut cyc_idx = idx3;
        cyc_idx.embeds_adj.insert(30, vec![31]);
        cyc_idx.embeds_adj.insert(31, vec![30]);
        assert!(matches!(
            resolve_hint_type_d(&cyc_idx, &iref, &fld8, 0),
            ResolvedType::Unknown
        ));
        assert!(matches!(
            resolve_hint_type_d(&cyc_idx, &iref, "field:type:IfaceA|IfaceB", 0),
            ResolvedType::Workspace(s) if s.id == 31
        ));
    }

    fn idx_probe() -> Index<'static> {
        static EMPTY: std::sync::OnceLock<ResolveInput> = std::sync::OnceLock::new();
        let inp = EMPTY.get_or_init(ResolveInput::default);
        Index::build(inp)
    }
}
