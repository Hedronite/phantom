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

#[test]
fn w17_kinds_reasons_links_and_display_helpers_round_trip() {
    use phantom_fractal_lang::{
        Extract, ExtractError, FileModule, Language, RefKind, ResolveInput, Resolver,
        format_file_display, format_qual_name, parse_qual_input,
    };

    for (item, symbol, text) in [
        (ItemKind::Function, SymbolKind::Function, "function"),
        (ItemKind::Struct, SymbolKind::Struct, "struct"),
        (ItemKind::Enum, SymbolKind::Enum, "enum"),
        (ItemKind::Trait, SymbolKind::Trait, "trait"),
        (ItemKind::Module, SymbolKind::Module, "module"),
        (ItemKind::Interface, SymbolKind::Interface, "interface"),
        (ItemKind::TypeAlias, SymbolKind::TypeAlias, "type_alias"),
        (ItemKind::Const, SymbolKind::Const, "const"),
        (ItemKind::Var, SymbolKind::Var, "var"),
    ] {
        assert_eq!(item.as_str(), text);
        assert_eq!(ItemKind::from_store_str(text), Some(item));
        assert_eq!(SymbolKind::from(item), symbol);
        assert_eq!(symbol.as_str(), text);
        assert_eq!(SymbolKind::from_store_str(text), Some(symbol));
    }
    assert_eq!(SymbolKind::Method.as_str(), "method");
    assert_eq!(
        SymbolKind::from_store_str("method"),
        Some(SymbolKind::Method)
    );
    assert_eq!(ItemKind::from_store_str("unknown"), None);
    assert_eq!(SymbolKind::from_store_str("unknown"), None);

    for (kind, text) in [
        (RefKind::Call, "call"),
        (RefKind::MethodCall, "method_call"),
        (RefKind::Macro, "macro"),
        (RefKind::MacroArg, "macro_arg"),
        (RefKind::Embed, "embed"),
        (RefKind::ValueRef, "value_ref"),
    ] {
        assert_eq!(kind.as_str(), text);
        assert_eq!(RefKind::from_store_str(text), Some(kind));
    }
    assert_eq!(RefKind::from_store_str("unknown"), None);

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
        (Reason::PackageImport, "package-import"),
        (Reason::EmbeddedMethod, "embedded-method"),
        (Reason::InterfaceMethod, "interface-method"),
    ] {
        assert_eq!(reason.as_str(), text);
        assert_eq!(Reason::from_store_str(text), Some(reason));
    }
    assert_eq!(Reason::from_store_str("unknown"), None);

    assert_eq!(LinkKind::Embeds.as_str(), "embeds");
    assert_eq!(LinkReason::StructuralImpl.as_str(), "structural-impl");
    assert_eq!(LinkReason::EmbeddedType.as_str(), "embedded-type");

    let fm = FileModule {
        file: "pkg/a.go".to_owned(),
        module: 42,
    };
    assert_eq!(fm.file, "pkg/a.go");
    assert_eq!(fm.module, 42);

    assert_eq!(format_qual_name("rust", "Type::Method"), "Type::Method");
    assert_eq!(format_qual_name("go", "Type::Method"), "Type.Method");
    assert_eq!(
        format_file_display("rust", "src/a.rs", "Type::Method"),
        "src/a.rs::Type::Method"
    );
    assert_eq!(
        format_file_display("go", "internal/x/f.go", "Type::Method"),
        "internal/x/f.go::Type.Method"
    );
    assert_eq!(parse_qual_input("rust", "a.b"), "a.b");
    assert_eq!(parse_qual_input("go", "Type.Method"), "Type::Method");

    struct DummyLang;
    impl Language for DummyLang {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["dm"]
        }
        fn extract(&self, _source: &str) -> Result<Extract, ExtractError> {
            Ok(Extract::default())
        }
    }
    struct DummyResolver;
    impl Resolver for DummyResolver {
        fn name(&self) -> &'static str {
            "dummy"
        }
        fn resolve(
            &self,
            _input: &ResolveInput,
        ) -> (
            Vec<phantom_fractal_lang::Edge>,
            phantom_fractal_lang::ResolveCounts,
        ) {
            (
                vec![phantom_fractal_lang::Edge {
                    ref_id: 1,
                    source: 2,
                    target: 3,
                    confidence: 1.0,
                    reason: Reason::PackageImport,
                }],
                phantom_fractal_lang::ResolveCounts {
                    unresolved: 1,
                    ..Default::default()
                },
            )
        }
    }
    let lang = DummyLang;
    assert_eq!(lang.name(), "dummy");
    assert_eq!(lang.extensions(), &["dm"]);
    assert!(lang.extract("").expect("extract").items.is_empty());
    assert_eq!(lang.qualifier_separator(), "::");
    assert!(!lang.has_parse_errors("fn main() {}"));
    let resolver = DummyResolver;
    assert_eq!(resolver.name(), "dummy");
    let empty_input = ResolveInput::default();
    let graph = resolver.resolve_graph(&empty_input);
    assert_eq!(graph.calls.len(), 1);
    assert_eq!(graph.counts.unresolved, 1);
}
