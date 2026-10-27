//! The four-tier resolver over hand-built inputs (SPEC-w3 G2b).

use phantom_fractal_lang::{
    CallRef, Edge, Id, Module, Reason, RefKind, ResolveCounts, ResolveInput, Symbol, SymbolKind,
    Target, TargetKind, UseBinding,
};
use phantom_fractal_lang_rust::{confidence, resolve};

/// Builds a [`ResolveInput`] for one lib target rooted at `src/lib.rs`.
struct Input {
    input: ResolveInput,
    next: Id,
}

impl Input {
    fn new() -> Self {
        let mut this = Self {
            input: ResolveInput::default(),
            next: 100,
        };
        this.input.targets.push(Target {
            id: 1,
            package: "pkg".to_owned(),
            name: "pkg".to_owned(),
            kind: TargetKind::Lib,
            root: "src/lib.rs".to_owned(),
        });
        this.input.modules.push(Module {
            id: 1,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: "src/lib.rs".to_owned(),
            scope: String::new(),
        });
        this
    }

    fn id(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    fn module(&mut self, parent: Id, name: &str, file: &str, scope: &str) -> Id {
        let id = self.id();
        let parent_path = self
            .input
            .modules
            .iter()
            .find(|m| m.id == parent)
            .map(|m| m.path.clone())
            .expect("parent");
        self.input.modules.push(Module {
            id,
            target: 1,
            parent: Some(parent),
            path: format!("{parent_path}::{name}"),
            file: file.to_owned(),
            scope: scope.to_owned(),
        });
        id
    }

    fn symbol(&mut self, kind: SymbolKind, name: &str, file: &str, scope: &str) -> Id {
        let id = self.id();
        self.input.symbols.push(Symbol {
            id,
            kind,
            name: name.to_owned(),
            owner: None,
            owner_path: None,
            trait_name: None,
            scope: scope.to_owned(),
            module: None,
            file: file.to_owned(),
        });
        id
    }

    fn function(&mut self, name: &str) -> Id {
        self.symbol(SymbolKind::Function, name, "src/lib.rs", "")
    }

    fn method(&mut self, owner_path: &str, trait_name: Option<&str>, name: &str) -> Id {
        let id = self.symbol(SymbolKind::Method, name, "src/lib.rs", "");
        let symbol = self.input.symbols.last_mut().expect("symbol");
        symbol.owner = Some(
            owner_path
                .rsplit("::")
                .next()
                .unwrap_or(owner_path)
                .to_owned(),
        );
        symbol.owner_path = Some(owner_path.to_owned());
        symbol.trait_name = trait_name.map(str::to_owned);
        id
    }

    fn uses(&mut self, file: &str, scope: &str, path: &str, alias: Option<&str>, glob: bool) {
        self.input.uses.push(UseBinding {
            module: None,
            scope: scope.to_owned(),
            file: file.to_owned(),
            path: path.to_owned(),
            alias: alias.map(str::to_owned),
            glob,
        });
    }

    fn call_in(
        &mut self,
        from: Option<Id>,
        file: &str,
        scope: &str,
        text: &str,
        kind: RefKind,
    ) -> Id {
        let id = self.id();
        self.input.refs.push(CallRef {
            id,
            from,
            module: None,
            scope: scope.to_owned(),
            file: file.to_owned(),
            kind,
            text: text.to_owned(),
            shadowed: false,
        });
        id
    }

    fn call(&mut self, from: Id, text: &str) -> Id {
        self.call_in(Some(from), "src/lib.rs", "", text, RefKind::Call)
    }

    fn resolve(&self) -> (Vec<Edge>, ResolveCounts) {
        resolve(&self.input)
    }
}

fn edge(ref_id: Id, source: Id, target: Id, reason: Reason) -> Edge {
    Edge {
        ref_id,
        source,
        target,
        confidence: confidence(reason),
        reason,
    }
}

fn only_edge(input: &Input) -> Option<Edge> {
    let (edges, counts) = input.resolve();
    assert!(edges.len() <= 1, "{edges:?}");
    assert_eq!(counts.edges, edges.len() as u64);
    edges.into_iter().next()
}

#[test]
fn confidences_are_the_spec_constants() {
    assert_eq!(confidence(Reason::PathExact), 1.0);
    assert_eq!(confidence(Reason::Local), 0.95);
    assert_eq!(confidence(Reason::UseAlias), 0.95);
    assert_eq!(confidence(Reason::Assoc), 0.9);
}

#[test]
fn path_exact_walks_crate_self_and_super() {
    let mut i = Input::new();
    let util = i.module(1, "util", "src/util.rs", "");
    let helper = i.symbol(SymbolKind::Function, "helper", "src/util.rs", "");
    let caller = i.function("caller");
    let deep = i.symbol(SymbolKind::Function, "deep", "src/util.rs", "");
    let crate_call = i.call(caller, "crate::util::helper");
    let self_call = i.call(caller, "self::util::helper");
    let super_call = i.call_in(
        Some(deep),
        "src/util.rs",
        "",
        "super::caller",
        RefKind::Call,
    );
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(crate_call, caller, helper, Reason::PathExact),
            edge(self_call, caller, helper, Reason::PathExact),
            edge(super_call, deep, caller, Reason::PathExact),
        ]
    );
    assert_eq!(counts.edges, 3);
    let _ = util;
}

#[test]
fn local_finds_an_item_or_a_child_module_in_the_call_scope() {
    let mut i = Input::new();
    i.module(1, "util", "src/util.rs", "");
    let helper = i.symbol(SymbolKind::Function, "helper", "src/util.rs", "");
    let near = i.function("near");
    let caller = i.function("caller");
    let item = i.call(caller, "near");
    let through = i.call(caller, "util::helper");
    let generic = i.call(caller, "near::<u8>");
    let (edges, _) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(item, caller, near, Reason::Local),
            edge(through, caller, helper, Reason::Local),
            edge(generic, caller, near, Reason::Local),
        ]
    );
}

#[test]
fn use_alias_follows_a_use_or_its_alias() {
    let mut i = Input::new();
    i.module(1, "util", "src/util.rs", "");
    let helper = i.symbol(SymbolKind::Function, "helper", "src/util.rs", "");
    i.uses("src/lib.rs", "", "crate::util::helper", None, false);
    i.uses("src/lib.rs", "", "util::helper", Some("renamed"), false);
    i.uses("src/lib.rs", "", "crate::util", Some("u"), false);
    let caller = i.function("caller");
    let plain = i.call(caller, "helper");
    let renamed = i.call(caller, "renamed");
    let through = i.call(caller, "u::helper");
    let (edges, _) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(plain, caller, helper, Reason::UseAlias),
            edge(renamed, caller, helper, Reason::UseAlias),
            edge(through, caller, helper, Reason::UseAlias),
        ]
    );
}

#[test]
fn assoc_needs_exactly_one_inherent_method() {
    let mut i = Input::new();
    let ledger = i.symbol(SymbolKind::Struct, "Ledger", "src/lib.rs", "");
    let new = i.method("Ledger<T>", None, "new");
    i.method("Ledger", Some("Default"), "new");
    i.method("Ledger", Some("Make"), "make");
    i.method("crate::Ledger", None, "open");
    i.method("crate::Ledger", None, "open");
    let caller = i.function("caller");
    let assoc = i.call(caller, "Ledger::new");
    i.call(caller, "Ledger::make");
    i.call(caller, "Ledger::open");
    let path = i.call(caller, "crate::Ledger::new");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(assoc, caller, new, Reason::Assoc),
            edge(path, caller, new, Reason::PathExact),
        ]
    );
    // `make` is only a trait method; `open` has two inherent candidates.
    assert_eq!(counts.unresolved, 2);
    let _ = ledger;
}

#[test]
fn two_candidates_give_no_edge() {
    let mut i = Input::new();
    i.function("twin");
    i.function("twin");
    let caller = i.function("caller");
    i.call(caller, "twin");
    assert_eq!(only_edge(&i), None);
    assert_eq!(i.resolve().1.unresolved, 1);
}

#[test]
fn a_shadowed_call_gives_no_edge() {
    let mut i = Input::new();
    i.function("helper");
    let caller = i.function("caller");
    i.call(caller, "helper");
    i.input.refs[0].shadowed = true;
    assert_eq!(only_edge(&i), None);
    assert_eq!(i.resolve().1.unresolved, 1);
}

#[test]
fn a_name_only_a_glob_supplies_gives_no_edge() {
    let mut i = Input::new();
    let tests = i.module(1, "tests", "src/lib.rs", "tests");
    i.function("helper");
    i.uses("src/lib.rs", "tests", "super", None, true);
    let check = i.symbol(SymbolKind::Function, "check", "src/lib.rs", "tests");
    i.call_in(Some(check), "src/lib.rs", "tests", "helper", RefKind::Call);
    let through = i.call_in(
        Some(check),
        "src/lib.rs",
        "tests",
        "super::helper",
        RefKind::Call,
    );
    let (edges, counts) = i.resolve();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].ref_id, through);
    assert_eq!(counts.unresolved, 1);
    let _ = tests;
}

#[test]
fn a_local_item_beats_a_use_binding() {
    let mut i = Input::new();
    i.module(1, "util", "src/util.rs", "");
    i.symbol(SymbolKind::Function, "helper", "src/util.rs", "");
    let local = i.function("helper");
    i.uses("src/lib.rs", "", "crate::util::helper", None, false);
    let caller = i.function("caller");
    let call = i.call(caller, "helper");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, caller, local, Reason::Local))
    );
}

#[test]
fn counts_cover_every_reference_kind() {
    let mut i = Input::new();
    i.function("helper");
    let caller = i.function("caller");
    i.call(caller, "helper");
    i.call(caller, "std::mem::drop");
    i.call(caller, "core::mem::swap");
    i.call(caller, "serde::de::from_str");
    i.call(caller, "::alloc::vec::Vec::new");
    i.call(caller, "missing");
    i.call(caller, "crate::missing");
    i.call(caller, "<T as Trait>::f");
    i.call_in(None, "src/lib.rs", "", "helper", RefKind::Call);
    i.call_in(Some(caller), "src/lib.rs", "", "len", RefKind::MethodCall);
    i.call_in(Some(caller), "src/lib.rs", "", "println", RefKind::Macro);
    i.call_in(Some(caller), "src/orphan.rs", "", "helper", RefKind::Call);
    let (_, counts) = i.resolve();
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 1,
            unresolved: 3,
            external: 5,
            no_source: 1,
            method_calls: 1,
            macros: 1,
        }
    );
}

#[test]
fn inline_modules_nest_by_scope() {
    let mut i = Input::new();
    let outer = i.module(1, "outer", "src/lib.rs", "outer");
    i.module(outer, "inner", "src/lib.rs", "outer::inner");
    let deep = i.symbol(SymbolKind::Function, "deep", "src/lib.rs", "outer::inner");
    let top = i.function("top");
    let caller = i.symbol(SymbolKind::Function, "caller", "src/lib.rs", "outer::inner");
    let local = i.call_in(
        Some(caller),
        "src/lib.rs",
        "outer::inner",
        "deep",
        RefKind::Call,
    );
    let up = i.call_in(
        Some(caller),
        "src/lib.rs",
        "outer::inner",
        "super::super::top",
        RefKind::Call,
    );
    i.call_in(
        Some(caller),
        "src/lib.rs",
        "outer::inner",
        "top",
        RefKind::Call,
    );
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(local, caller, deep, Reason::Local),
            edge(up, caller, top, Reason::PathExact),
        ]
    );
    // `top` is not declared in `outer::inner`, and nothing there imports it.
    assert_eq!(counts.external, 1);
}

#[test]
fn a_use_cycle_ends_without_an_edge() {
    let mut i = Input::new();
    i.uses("src/lib.rs", "", "self::b", Some("a"), false);
    i.uses("src/lib.rs", "", "self::a", Some("b"), false);
    let caller = i.function("caller");
    i.call(caller, "a");
    assert_eq!(only_edge(&i), None);
    assert_eq!(i.resolve().1.unresolved, 1);
}

#[test]
fn a_tuple_struct_or_a_module_without_a_file_gives_no_edge() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Wrapper", "src/lib.rs", "");
    i.symbol(SymbolKind::Module, "ghost", "src/lib.rs", "");
    let caller = i.function("caller");
    i.call(caller, "Wrapper");
    i.call(caller, "ghost::f");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty());
    assert_eq!(counts.unresolved, 2);
}

#[test]
fn resolve_is_pure() {
    let mut i = Input::new();
    i.function("helper");
    let caller = i.function("caller");
    i.call(caller, "helper");
    assert_eq!(i.resolve(), i.resolve());
}
