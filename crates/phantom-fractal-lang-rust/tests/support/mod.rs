//! A hand-built [`ResolveInput`] for the resolver tests.
// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use phantom_fractal_lang::{
    CallRef, Dep, Edge, Id, Module, Reason, RefKind, ResolveCounts, ResolveInput, Symbol,
    SymbolKind, Target, TargetKind, UseBinding,
};
use phantom_fractal_lang_rust::{confidence, resolve};

/// Builds a [`ResolveInput`] for one lib target rooted at `src/lib.rs`.
pub struct Input {
    pub input: ResolveInput,
    next: Id,
}

impl Input {
    pub fn new() -> Self {
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

    /// Adds a target of `package` rooted at `root`, and its root module.
    /// Returns the root module's id.
    pub fn target(&mut self, package: &str, kind: TargetKind, root: &str) -> Id {
        let target = self.id();
        let name = match kind {
            TargetKind::Lib => package,
            TargetKind::Bin | TargetKind::Test => root
                .rsplit('/')
                .next()
                .unwrap_or(root)
                .trim_end_matches(".rs"),
        };
        self.input.targets.push(Target {
            id: target,
            package: package.to_owned(),
            name: name.to_owned(),
            kind,
            root: root.to_owned(),
        });
        let module = self.id();
        self.input.modules.push(Module {
            id: module,
            target,
            parent: None,
            path: "crate".to_owned(),
            file: root.to_owned(),
            scope: String::new(),
        });
        module
    }

    /// Lets `package` call the lib of workspace package `on`.
    pub fn dep(&mut self, package: &str, on: &str) {
        self.push_dep(package, on, false);
    }

    /// Lets the test targets of `package` call the lib of `on`.
    pub fn dev_dep(&mut self, package: &str, on: &str) {
        self.push_dep(package, on, true);
    }

    fn push_dep(&mut self, package: &str, on: &str, dev: bool) {
        self.input.deps.push(Dep {
            package: package.to_owned(),
            on: on.to_owned(),
            crate_name: on.replace('-', "_"),
            dev,
        });
    }

    pub fn id(&mut self) -> Id {
        self.next += 1;
        self.next
    }

    pub fn module(&mut self, parent: Id, name: &str, file: &str, scope: &str) -> Id {
        let id = self.id();
        let (parent_path, target) = self
            .input
            .modules
            .iter()
            .find(|m| m.id == parent)
            .map(|m| (m.path.clone(), m.target))
            .expect("parent");
        self.input.modules.push(Module {
            id,
            target,
            parent: Some(parent),
            path: format!("{parent_path}::{name}"),
            file: file.to_owned(),
            scope: scope.to_owned(),
        });
        id
    }

    pub fn symbol(&mut self, kind: SymbolKind, name: &str, file: &str, scope: &str) -> Id {
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
            signature: None,
        });
        id
    }

    pub fn function(&mut self, name: &str) -> Id {
        self.symbol(SymbolKind::Function, name, "src/lib.rs", "")
    }

    pub fn method(&mut self, owner_path: &str, trait_name: Option<&str>, name: &str) -> Id {
        self.method_in("src/lib.rs", owner_path, trait_name, name)
    }

    pub fn method_in(
        &mut self,
        file: &str,
        owner_path: &str,
        trait_name: Option<&str>,
        name: &str,
    ) -> Id {
        let id = self.symbol(SymbolKind::Method, name, file, "");
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

    pub fn uses(&mut self, file: &str, scope: &str, path: &str, alias: Option<&str>, glob: bool) {
        self.use_leaf(file, scope, path, alias, glob);
    }

    /// Adds a `use` leaf and returns its id.
    pub fn use_leaf(
        &mut self,
        file: &str,
        scope: &str,
        path: &str,
        alias: Option<&str>,
        glob: bool,
    ) -> Id {
        let id = self.id();
        self.input.uses.push(UseBinding {
            id,
            module: None,
            scope: scope.to_owned(),
            file: file.to_owned(),
            path: path.to_owned(),
            alias: alias.map(str::to_owned),
            glob,
        });
        id
    }

    pub fn call_in(
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
            receiver: None,
            receiver_type: None,
            shadowed: false,
        });
        id
    }

    pub fn call(&mut self, from: Id, text: &str) -> Id {
        self.call_in(Some(from), "src/lib.rs", "", text, RefKind::Call)
    }

    /// A `Call` of `text` from `from`, which sits in `file`.
    pub fn call_from(&mut self, from: Id, file: &str, text: &str) -> Id {
        self.call_in(Some(from), file, "", text, RefKind::Call)
    }

    /// A method call `receiver.name()` in `src/lib.rs`.
    pub fn method_call(&mut self, from: Option<Id>, receiver: &str, name: &str) -> Id {
        let id = self.call_in(from, "src/lib.rs", "", name, RefKind::MethodCall);
        self.input.refs.last_mut().expect("ref").receiver = Some(receiver.to_owned());
        id
    }

    /// A method call `receiver.name()` in `src/lib.rs` with the receiver
    /// type hint `hint`.
    pub fn typed_call(&mut self, from: Id, receiver: &str, name: &str, hint: &str) -> Id {
        let id = self.method_call(Some(from), receiver, name);
        self.input.refs.last_mut().expect("ref").receiver_type = Some(hint.to_owned());
        id
    }

    /// Sets the stored signature of symbol `id`.
    pub fn signature(&mut self, id: Id, signature: &str) {
        let symbol = self
            .input
            .symbols
            .iter_mut()
            .find(|symbol| symbol.id == id)
            .expect("symbol");
        symbol.signature = Some(signature.to_owned());
    }

    pub fn resolve(&self) -> (Vec<Edge>, ResolveCounts) {
        resolve(&self.input)
    }
}

pub fn edge(ref_id: Id, source: Id, target: Id, reason: Reason) -> Edge {
    Edge {
        ref_id,
        source,
        target,
        confidence: confidence(reason),
        reason,
    }
}

pub fn only_edge(input: &Input) -> Option<Edge> {
    let (edges, counts) = input.resolve();
    assert!(edges.len() <= 1, "{edges:?}");
    assert_eq!(counts.edges, edges.len() as u64);
    edges.into_iter().next()
}
