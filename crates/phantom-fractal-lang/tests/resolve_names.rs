//! The resolve types' store texts and conversions.
//!
//! The store keeps these texts in `edges.kind`, `edges.reason`,
//! `edges.src_kind` and `edges.dst_kind`, so each one is fixed here as the
//! documentation names it.

use phantom_fractal_lang::{ItemKind, LinkKind, LinkReason, Node, Reason, SymbolKind};

#[test]
fn every_item_kind_is_the_symbol_kind_of_the_same_name() {
    for (item, symbol) in [
        (ItemKind::Function, SymbolKind::Function),
        (ItemKind::Struct, SymbolKind::Struct),
        (ItemKind::Enum, SymbolKind::Enum),
        (ItemKind::Trait, SymbolKind::Trait),
        (ItemKind::Module, SymbolKind::Module),
    ] {
        assert_eq!(SymbolKind::from(item), symbol);
    }
}

#[test]
fn each_call_reason_has_its_store_text() {
    for (reason, text) in [
        (Reason::PathExact, "path-exact"),
        (Reason::Local, "local"),
        (Reason::UseAlias, "use-alias"),
        (Reason::Assoc, "assoc"),
        (Reason::SelfMethod, "self-method"),
        (Reason::SelfAssoc, "self-assoc"),
        (Reason::CratePath, "crate-path"),
        (Reason::TypedMethod, "typed-method"),
        (Reason::UniqueMethod, "unique-method"),
    ] {
        assert_eq!(reason.as_str(), text);
    }
}

#[test]
fn a_link_reason_shares_the_text_of_the_call_reason_of_its_tier() {
    for (link, call) in [
        (LinkReason::PathExact, Reason::PathExact),
        (LinkReason::Local, Reason::Local),
        (LinkReason::UseAlias, Reason::UseAlias),
        (LinkReason::CratePath, Reason::CratePath),
    ] {
        assert_eq!(link.as_str(), call.as_str());
    }
    assert_eq!(LinkReason::ImplTrait.as_str(), "impl-trait");
}

#[test]
fn each_link_kind_has_its_store_text() {
    assert_eq!(LinkKind::Import.as_str(), "import");
    assert_eq!(LinkKind::Impl.as_str(), "impl");
}

#[test]
fn a_node_gives_its_row_id_and_its_row_kind() {
    assert_eq!(Node::Module(7).id(), 7);
    assert_eq!(Node::Symbol(9).id(), 9);
    assert_eq!(Node::Module(7).kind(), "module");
    assert_eq!(Node::Symbol(9).kind(), "symbol");
}
