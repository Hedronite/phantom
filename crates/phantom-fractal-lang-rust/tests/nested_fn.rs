//! Nested inner `fn` extraction and scoped call resolution (SPEC-w16 G2).

use phantom_fractal_lang::{
    CallRef, Extract, Id, Language, Module, Reason, ResolveInput, Symbol, SymbolKind, Target,
    TargetKind,
};
use phantom_fractal_lang_rust::{MAX_NESTED_FN_DEPTH, RustLanguage, resolve};

fn extract(source: &str) -> Extract {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
}

fn edge_names(source: &str) -> Vec<(String, String, Reason)> {
    let ext = extract(source);
    let mut input = ResolveInput {
        targets: vec![Target {
            id: 1,
            package: "pkg".to_owned(),
            name: "pkg".to_owned(),
            kind: TargetKind::Lib,
            root: "src/lib.rs".to_owned(),
        }],
        modules: vec![Module {
            id: 1,
            target: 1,
            parent: None,
            path: "crate".to_owned(),
            file: "src/lib.rs".to_owned(),
            scope: String::new(),
        }],
        symbols: Vec::new(),
        uses: Vec::new(),
        refs: Vec::new(),
        deps: Vec::new(),
    };
    let mut callers = std::collections::HashMap::new();
    let mut next: Id = 10;
    for (idx, item) in ext.items.iter().enumerate() {
        let owner = ext
            .item_owners
            .get(idx)
            .and_then(Option::as_deref)
            .map(str::to_owned);
        callers.insert((None, item.name.clone(), item.start_line), next);
        input.symbols.push(Symbol {
            id: next,
            kind: SymbolKind::from(item.kind),
            name: item.name.clone(),
            owner,
            owner_path: None,
            trait_name: None,
            scope: ext.item_scopes[idx].clone(),
            module: Some(1),
            signature: Some(item.signature.clone()),
            file: "src/lib.rs".to_owned(),
        });
        next += 1;
    }
    for method in &ext.methods {
        callers.insert(
            (
                Some(method.owner.clone()),
                method.name.clone(),
                method.start_line,
            ),
            next,
        );
        input.symbols.push(Symbol {
            id: next,
            kind: SymbolKind::Method,
            name: method.name.clone(),
            owner: Some(method.owner.clone()),
            owner_path: Some(method.owner_path.clone()),
            trait_name: method.trait_name.clone(),
            scope: method.scope.clone(),
            module: Some(1),
            signature: Some(method.signature.clone()),
            file: "src/lib.rs".to_owned(),
        });
        next += 1;
    }
    for (idx, r) in ext.refs.iter().enumerate() {
        let from = r.from.as_ref().and_then(|c| {
            callers
                .get(&(c.owner.clone(), c.name.clone(), c.start_line))
                .copied()
        });
        input.refs.push(CallRef {
            id: Id::try_from(idx + 1).expect("id"),
            from,
            module: Some(1),
            scope: r.scope.clone(),
            file: "src/lib.rs".to_owned(),
            kind: r.kind,
            text: r.text.clone(),
            receiver: r.receiver.clone(),
            receiver_type: r.receiver_type.clone(),
            shadowed: r.shadowed,
        });
    }
    let names: std::collections::HashMap<Id, String> = input
        .symbols
        .iter()
        .map(|s| {
            let q = match &s.owner {
                Some(owner) => format!("{owner}::{}", s.name),
                None => s.name.clone(),
            };
            (s.id, q)
        })
        .collect();
    let (edges, _) = resolve(&input);
    edges
        .into_iter()
        .map(|e| (names[&e.source].clone(), names[&e.target].clone(), e.reason))
        .collect()
}

#[test]
fn extracts_nested_fns_with_enclosing_owners_and_excludes_trait_and_local_impls() {
    let src = r#"
pub struct S;
impl S {
    pub fn run(&self) {
        for _ in 0..1 {
            fn step() {}
        }
        let _c = || {
            fn in_closure() {}
        };
    }
}
pub trait T {
    fn def_body(&self) {
        fn ignored_in_trait() {}
    }
}
pub fn outer() {
    struct Local;
    impl Local {
        fn local_method() {
            fn ignored_in_local_impl() {}
        }
    }
    fn mid() {
        /// Leaf doc.
        fn leaf() {}
    }
}
"#;
    let got = extract(src);
    let pairs: Vec<(&str, Option<&str>)> = got
        .items
        .iter()
        .zip(&got.item_owners)
        .map(|(i, o)| (i.name.as_str(), o.as_deref()))
        .collect();
    assert!(pairs.contains(&("step", Some("S::run"))), "{pairs:?}");
    assert!(pairs.contains(&("in_closure", Some("S::run"))), "{pairs:?}");
    assert!(pairs.contains(&("mid", Some("outer"))), "{pairs:?}");
    assert!(pairs.contains(&("leaf", Some("outer::mid"))), "{pairs:?}");
    assert!(
        !pairs.iter().any(|(n, _)| *n == "ignored_in_trait"),
        "{pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(n, _)| *n == "local_method"),
        "{pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(n, _)| *n == "ignored_in_local_impl"),
        "{pairs:?}"
    );

    let leaf = got.items.iter().find(|i| i.name == "leaf").expect("leaf");
    assert_eq!(leaf.doc_summary.as_deref(), Some("Leaf doc."));
    assert_eq!(leaf.signature, "fn leaf()");
}

#[test]
fn resolves_nested_calls_scoping_twins_shadowing_and_depth_two() {
    let src = r#"
pub fn aux() {}
pub fn shared() { aux(); }
pub fn outer_a() {
    fn shared() { aux(); }
    shared();
}
pub fn outer_b() {
    fn shared() { aux(); }
    shared();
}
pub fn outer_shadowed() {
    fn shared() {}
    let shared = || ();
    shared();
}
pub fn layered() {
    fn mid() {
        fn leaf() {
            aux();
            mid();
        }
        leaf();
    }
    mid();
    leaf();
}
pub struct App;
impl App {
    pub fn nearby_entries(&self) {
        fn is_root() { aux(); }
        is_root();
    }
}
"#;
    let edges = edge_names(src);
    let has = |from: &str, to: &str| {
        edges
            .iter()
            .any(|(f, t, r)| f == from && t == to && *r == Reason::Local)
    };
    assert!(has("outer_a", "outer_a::shared"), "{edges:?}");
    assert!(has("outer_a::shared", "aux"), "{edges:?}");
    assert!(has("outer_b", "outer_b::shared"), "{edges:?}");
    assert!(has("outer_b::shared", "aux"), "{edges:?}");
    assert!(has("shared", "aux"), "{edges:?}");
    assert!(has("layered", "layered::mid"), "{edges:?}");
    assert!(has("layered::mid", "layered::mid::leaf"), "{edges:?}");
    assert!(has("layered::mid::leaf", "aux"), "{edges:?}");
    assert!(has("layered::mid::leaf", "layered::mid"), "{edges:?}");
    assert!(
        has("App::nearby_entries", "App::nearby_entries::is_root"),
        "{edges:?}"
    );
    assert!(has("App::nearby_entries::is_root", "aux"), "{edges:?}");

    // Shadowed `let shared = || ()` does not link to `outer_shadowed::shared`.
    assert!(
        !edges.iter().any(|(f, _, _)| f == "outer_shadowed"),
        "{edges:?}"
    );
    // Outer `layered` cannot call private depth-2 `leaf` directly.
    assert!(
        !edges
            .iter()
            .any(|(f, t, _)| f == "layered" && t == "layered::mid::leaf"),
        "{edges:?}"
    );
}

#[test]
fn inline_mod_nested_fn_preserves_module_scope_and_duplicate_twins_stay_unresolved() {
    let src = r#"
macro_rules! gen_fn {
    () => {
        fn inside_macro() {}
    };
}
pub mod sub {
    pub fn helper() {}
    pub fn outer() {
        fn inner() { helper(); }
        inner();
    }
}
pub fn dup(c: bool) {
    if c {
        fn twin() {}
    } else {
        fn twin() {}
    }
    twin();
}
"#;
    let ext = extract(src);
    let inner_idx = ext
        .items
        .iter()
        .position(|i| i.name == "inner")
        .expect("inner");
    assert_eq!(ext.item_scopes[inner_idx], "sub");
    assert_eq!(ext.item_owners[inner_idx].as_deref(), Some("outer"));
    assert!(!ext.items.iter().any(|i| i.name == "inside_macro"));
    let broken = extract("fn () { fn inner_broken() {} }");
    assert!(!broken.items.iter().any(|i| i.name == "inner_broken"));
    let in_const =
        extract("const _: () = { fn inside_const() {} struct InsideConst; mod hidden {} };");
    assert!(!in_const.items.iter().any(|i| i.name == "inside_const"));
    assert!(!in_const.items.iter().any(|i| i.name == "InsideConst"));
    assert!(!in_const.items.iter().any(|i| i.name == "hidden"));
    assert!(in_const.mod_decls.is_empty());

    let edges = edge_names(src);
    assert!(!edges.iter().any(|(f, _, _)| f == "dup"), "{edges:?}");
}

#[test]
fn method_body_does_not_climb_to_free_fn_named_after_impl_type() {
    let src = r#"
pub struct App {}
#[allow(non_snake_case)]
pub fn App() {
    fn run() {}
    run();
}
pub fn run() {}
impl App {
    pub fn go(&self) {
        run();
    }
    pub fn with_inner(&self) {
        fn step() {
            run();
        }
        step();
    }
}
"#;
    let edges = edge_names(src);
    assert!(
        edges
            .iter()
            .any(|(f, t, r)| f == "App::go" && t == "run" && *r == Reason::Local),
        "{edges:?}"
    );
    assert!(
        !edges
            .iter()
            .any(|(f, t, _)| f == "App::go" && t == "App::run"),
        "{edges:?}"
    );
    assert!(
        edges
            .iter()
            .any(|(f, t, _)| f == "App::with_inner::step" && t == "run"),
        "{edges:?}"
    );
}

#[test]
fn deep_nesting_is_capped_and_extracts_fast() {
    let depth = 1_000usize;
    let mut src = String::new();
    for i in 0..depth {
        src.push_str(&format!(
            "fn f{i}() {{
"
        ));
    }
    for i in (0..depth).rev() {
        src.push_str(&format!(
            "f{i}();
}}
"
        ));
    }
    let started = std::time::Instant::now();
    let got = extract(&src);
    let elapsed = started.elapsed();
    assert_eq!(got.items.len(), 1 + MAX_NESTED_FN_DEPTH);
    assert!(
        elapsed < std::time::Duration::from_millis(250),
        "depth-{depth} extraction took {elapsed:?}"
    );
}
