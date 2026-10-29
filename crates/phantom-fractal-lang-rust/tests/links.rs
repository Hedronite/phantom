//! `import` and `impl` links over hand-built inputs (SPEC-w7 G0a).

mod support;

use phantom_fractal_lang::{Link, LinkKind, LinkReason, Node, SymbolKind, TargetKind};
use phantom_fractal_lang_rust::{links, resolve, resolve_graph};
use support::Input;

/// The root module of [`Input::new`]'s lib.
const ROOT: i64 = 1;

fn import(ref_id: i64, source: i64, target: Node, reason: LinkReason, confidence: f64) -> Link {
    Link {
        kind: LinkKind::Import,
        source: Node::Module(source),
        target,
        confidence,
        reason,
        ref_id: Some(ref_id),
    }
}

fn implements(ty: i64, trait_id: i64, confidence: f64) -> Link {
    Link {
        kind: LinkKind::Impl,
        source: Node::Symbol(ty),
        target: Node::Symbol(trait_id),
        confidence,
        reason: LinkReason::ImplTrait,
        ref_id: None,
    }
}

#[test]
fn an_import_links_whatever_it_binds_and_an_underscore_alias_still_links() {
    let mut i = Input::new();
    let util = i.module(ROOT, "util", "src/util.rs", "");
    let show = i.symbol(SymbolKind::Trait, "Show", "src/util.rs", "");
    let plain = i.use_leaf("src/lib.rs", "", "crate::util::Show", None, false);
    let hidden = i.use_leaf("src/lib.rs", "", "util::Show", Some("_"), false);
    let module = i.use_leaf("src/lib.rs", "", "self::util", None, false);
    assert_eq!(
        links(&i.input),
        [
            import(plain, ROOT, Node::Symbol(show), LinkReason::PathExact, 1.0),
            import(hidden, ROOT, Node::Symbol(show), LinkReason::Local, 0.95),
            import(module, ROOT, Node::Module(util), LinkReason::PathExact, 1.0),
        ]
    );
}

#[test]
fn a_glob_an_orphan_a_variant_or_two_names_give_no_import() {
    let mut i = Input::new();
    i.module(ROOT, "util", "src/util.rs", "");
    i.symbol(SymbolKind::Enum, "Mode", "src/util.rs", "");
    i.symbol(SymbolKind::Struct, "Pair", "src/util.rs", "");
    i.symbol(SymbolKind::Function, "Pair", "src/util.rs", "");
    i.symbol(SymbolKind::Function, "f", "src/util.rs", "");
    i.use_leaf("src/lib.rs", "", "crate::util", None, true);
    i.use_leaf("src/stray.rs", "", "crate::util::f", None, false);
    i.use_leaf("src/lib.rs", "", "crate::util::Mode::On", None, false);
    i.use_leaf("src/lib.rs", "", "crate::util::Pair", None, false);
    i.use_leaf("src/lib.rs", "", "std::fmt", None, false);
    i.use_leaf("src/lib.rs", "", "crate::util::gone", None, false);
    assert_eq!(links(&i.input), []);
}

#[test]
fn an_impl_links_its_owner_to_the_one_trait_once_with_the_first_tier() {
    let mut i = Input::new();
    i.module(ROOT, "traits", "src/traits.rs", "");
    let named = i.symbol(SymbolKind::Trait, "Named", "src/traits.rs", "");
    let item = i.symbol(SymbolKind::Struct, "Item", "src/lib.rs", "");
    i.uses("src/lib.rs", "", "traits::Named", None, false);
    i.method("Item", Some("crate::traits::Named"), "name");
    i.method("Item<u8>", Some("Named<u8>"), "name");
    i.method("Item", Some("Named"), "label");
    let impls: Vec<Link> = links(&i.input)
        .into_iter()
        .filter(|link| link.kind == LinkKind::Impl)
        .collect();
    assert_eq!(impls, [implements(item, named, 1.0)]);
}

#[test]
fn no_impl_without_one_trait_or_an_owner_in_the_methods_target() {
    let mut i = Input::new();
    i.symbol(SymbolKind::Struct, "Item", "src/lib.rs", "");
    i.symbol(SymbolKind::Struct, "Other", "src/lib.rs", "");
    i.symbol(SymbolKind::Trait, "Tr", "src/lib.rs", "");
    // The trait path names a struct, nothing, or the std library.
    i.method("Item", Some("Other"), "a");
    i.method("Item", Some("Missing"), "b");
    i.method("Item", Some("std::fmt::Display"), "c");
    // The self type is a trait object, or no type.
    i.method("dyn Tr", Some("Tr"), "d");
    i.method("Gone", Some("Tr"), "e");
    // An inherent method.
    i.method("Other", None, "f");
    // A bin's impl on the lib's type: the owner is in another target.
    i.target("pkg", TargetKind::Bin, "src/main.rs");
    i.symbol(SymbolKind::Trait, "Speak", "src/main.rs", "");
    i.method_in("src/main.rs", "pkg::Other", Some("Speak"), "speak");
    assert_eq!(links(&i.input), []);
}

#[test]
fn links_leave_the_call_edges_and_counts_as_resolve_gives_them() {
    let mut i = Input::new();
    i.module(ROOT, "util", "src/util.rs", "");
    let helper = i.symbol(SymbolKind::Function, "helper", "src/util.rs", "");
    let caller = i.function("caller");
    i.uses("src/lib.rs", "", "util::helper", None, false);
    i.call(caller, "helper");
    i.call(caller, "util::gone");
    let graph = resolve_graph(&i.input);
    let (calls, counts) = resolve(&i.input);
    assert_eq!(graph.calls, calls);
    assert_eq!(graph.counts, counts);
    assert_eq!(graph.links, links(&i.input));
    assert_eq!(
        graph.links.iter().map(|l| l.target).collect::<Vec<_>>(),
        [Node::Symbol(helper)]
    );
}

#[test]
fn the_kind_texts_are_the_store_texts() {
    assert_eq!(LinkKind::Import.as_str(), "import");
    assert_eq!(LinkKind::Impl.as_str(), "impl");
    assert_eq!(Node::Module(3).kind(), "module");
    assert_eq!(Node::Symbol(4).kind(), "symbol");
    assert_eq!(Node::Symbol(4).id(), 4);
    let reasons = [
        LinkReason::PathExact,
        LinkReason::Local,
        LinkReason::UseAlias,
        LinkReason::CratePath,
        LinkReason::ImplTrait,
    ];
    assert_eq!(
        reasons.map(LinkReason::as_str),
        [
            "path-exact",
            "local",
            "use-alias",
            "crate-path",
            "impl-trait"
        ]
    );
}
