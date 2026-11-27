//! Completeness reasons, `exact`, and risk on constructed graphs
//! (SPEC-w5 G2c, G2d).

mod support;

use phantom_fractal_analyze::{CompletenessReason, ReasonCount, Risk};
use support::{Graph, star};

use CompletenessReason::{
    ExternalUsersPossible, MacroBoundary, ParseSkippedFiles, TraitDispatch, UnresolvedRefs,
};

fn target() -> Graph {
    let mut g = Graph::new();
    g.function(1, "t");
    g
}

fn reasons(g: &Graph) -> Vec<(CompletenessReason, Option<u64>)> {
    let got = g.impact(1);
    assert_eq!(got.exact, got.reasons.is_empty());
    got.reasons
        .iter()
        .map(|ReasonCount { reason, count }| (*reason, *count))
        .collect()
}

#[test]
fn a_private_function_with_nothing_unseen_is_exact() {
    let g = target();
    assert_eq!(reasons(&g), []);
    assert!(g.impact(1).exact);
}

#[test]
fn unresolved_refs_and_macro_boundary_count_refs_named_like_the_target() {
    let mut g = target();
    g.unresolved.insert("t".to_owned(), 0);
    g.macro_args.insert("t".to_owned(), 0);
    g.unresolved.insert("other".to_owned(), 5);
    assert_eq!(reasons(&g), []);
    g.unresolved.insert("t".to_owned(), 1);
    assert_eq!(reasons(&g), [(UnresolvedRefs, Some(1))]);
    g.unresolved.insert("t".to_owned(), 0);
    g.macro_args.insert("t".to_owned(), 3);
    assert_eq!(reasons(&g), [(MacroBoundary, Some(3))]);
}

#[test]
fn trait_dispatch_needs_a_method_with_a_trait() {
    let mut g = target();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    assert_eq!(reasons(&g), [], "a function with a trait name");
    g.facts_mut(1).kind = "method".to_owned();
    assert_eq!(reasons(&g), [(TraitDispatch, None)]);
    g.facts_mut(1).trait_name = None;
    assert_eq!(reasons(&g), [], "an inherent method");
}

#[test]
fn parse_skipped_files_applies_to_a_skip_or_an_unknown_count() {
    let mut g = target();
    g.rs_skipped = Some(0);
    assert_eq!(reasons(&g), []);
    g.rs_skipped = Some(2);
    assert_eq!(reasons(&g), [(ParseSkippedFiles, Some(2))]);
    g.rs_skipped = None;
    assert_eq!(reasons(&g), [(ParseSkippedFiles, None)]);
}

#[test]
fn external_users_possible_needs_pub_in_a_lib() {
    for (visibility, target_kind, applies) in [
        ("public", Some("lib"), true),
        ("public", Some("bin"), false),
        ("public", None, false),
        ("crate", Some("lib"), false),
        ("restricted:pub(super)", Some("lib"), false),
        ("private", Some("lib"), false),
    ] {
        let mut g = target();
        g.facts_mut(1).visibility = visibility.to_owned();
        g.facts_mut(1).target_kind = target_kind.map(str::to_owned);
        let want: &[(CompletenessReason, Option<u64>)] = if applies {
            &[(ExternalUsersPossible, None)]
        } else {
            &[]
        };
        assert_eq!(reasons(&g), want, "{visibility} {target_kind:?}");
    }
}

#[test]
fn every_reason_at_once_keeps_the_fixed_order() {
    let mut g = target();
    g.facts_mut(1).kind = "method".to_owned();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    g.facts_mut(1).visibility = "public".to_owned();
    g.unresolved.insert("t".to_owned(), 4);
    g.macro_args.insert("t".to_owned(), 2);
    g.rs_skipped = Some(7);
    assert_eq!(
        reasons(&g),
        [
            (UnresolvedRefs, Some(4)),
            (MacroBoundary, Some(2)),
            (TraitDispatch, None),
            (ParseSkippedFiles, Some(7)),
            (ExternalUsersPossible, None),
        ]
    );
    let names: Vec<&str> = CompletenessReason::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(
        names,
        [
            "unresolved_refs",
            "macro_boundary",
            "trait_dispatch",
            "parse_skipped_files",
            "external_users_possible",
            "stale_index",
            "unindexed_changes",
            "interface_dispatch",
            "value_reference",
            "build_excluded",
            "manifest_changed",
        ]
    );
    assert!(!g.impact(1).exact);
}

fn risk(direct: i64, second: i64) -> Risk {
    star(direct, second).impact(1).risk
}

/// `star(direct, 0)` with `unseen` unresolved refs named like the target,
/// split between unresolved and macro refs.
fn risk_unseen(direct: i64, unseen: i64) -> Risk {
    let mut g = star(direct, 0);
    g.unresolved.insert("t".to_owned(), unseen / 2);
    g.macro_args.insert("t".to_owned(), unseen - unseen / 2);
    g.impact(1).risk
}

#[test]
fn direct_boundaries_give_medium_high_and_critical() {
    assert_eq!(risk(0, 0), Risk::Low);
    assert_eq!(risk(2, 0), Risk::Low);
    assert_eq!(risk(3, 0), Risk::Medium);
    assert_eq!(risk(9, 0), Risk::Medium);
    assert_eq!(risk(10, 0), Risk::High);
    assert_eq!(risk(24, 0), Risk::High);
    assert_eq!(risk(25, 0), Risk::Critical);
}

#[test]
fn total_boundaries_give_medium_and_high() {
    // One direct caller plus `second` at depth 2.
    assert_eq!(star(1, 8).impact(1).counts.total, 9);
    assert_eq!(risk(1, 8), Risk::Low);
    assert_eq!(risk(1, 9), Risk::Medium);
    assert_eq!(risk(1, 48), Risk::Medium);
    assert_eq!(risk(1, 49), Risk::High);
}

#[test]
fn unknown_starts_where_unseen_reaches_max_one_direct() {
    assert_eq!(risk_unseen(4, 4), Risk::Unknown);
    assert_eq!(risk_unseen(4, 3), Risk::Medium);
    assert_eq!(risk_unseen(2, 2), Risk::Unknown);
    assert_eq!(risk_unseen(2, 1), Risk::Low);
    assert_eq!(risk_unseen(0, 1), Risk::Unknown);
    assert_eq!(risk_unseen(1, 1), Risk::Unknown);
    assert_eq!(risk_unseen(0, 0), Risk::Low);
    // Critical is checked first.
    assert_eq!(risk_unseen(25, 100), Risk::Critical);
    assert_eq!(risk_unseen(24, 24), Risk::Unknown);
    assert_eq!(risk_unseen(24, 23), Risk::High);
}

#[test]
fn only_unresolved_and_macro_counts_are_unseen() {
    // Null-count reasons and a skipped-file count never make it unknown.
    let mut g = star(1, 0);
    g.facts_mut(1).kind = "method".to_owned();
    g.facts_mut(1).trait_name = Some("Area".to_owned());
    g.facts_mut(1).visibility = "public".to_owned();
    g.rs_skipped = Some(50);
    let got = g.impact(1);
    assert_eq!(got.reasons.len(), 3);
    assert_eq!(got.risk, Risk::Low);
}

#[test]
fn tests_never_count_toward_risk() {
    let mut g = star(2, 0);
    for n in 0..30 {
        g.test(500 + n, &format!("test{n}"));
        g.call(500 + n, 1, 1.0);
    }
    let got = g.impact(1);
    assert_eq!(got.counts.tests, 30);
    assert_eq!(got.risk, Risk::Low);
    assert_eq!(Risk::Unknown.as_str(), "unknown");
}

#[test]
fn multilang_interface_value_build_manifest_and_go_external_rules() {
    use phantom_fractal_analyze::{DiffState, ImpactOptions, ImpactSource, impact_many};
    use phantom_fractal_store::{CallHit, StoreError, SymbolFacts};

    // Exercise default trait methods directly.
    struct MinimalSource(SymbolFacts);
    impl ImpactSource for MinimalSource {
        fn callers(&self, _id: i64) -> Result<Vec<CallHit>, StoreError> {
            Ok(Vec::new())
        }
        fn facts(&self, id: i64) -> Result<Option<SymbolFacts>, StoreError> {
            Ok((id == self.0.id).then(|| self.0.clone()))
        }
        fn unresolved_named(&self, _name: &str) -> Result<i64, StoreError> {
            Ok(0)
        }
        fn macro_args_named(&self, _name: &str) -> Result<i64, StoreError> {
            Ok(0)
        }
        fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
            Ok(Some(0))
        }
    }
    let minimal = MinimalSource(SymbolFacts {
        id: 1,
        name: "Fn".to_owned(),
        kind: "function".to_owned(),
        file: "src/a.rs".to_owned(),
        line: Some(1),
        visibility: "private".to_owned(),
        trait_name: None,
        is_test: false,
        package: Some("app".to_owned()),
        target_kind: Some("lib".to_owned()),
    });
    assert_eq!(minimal.file_lang("src/a.rs").unwrap(), None);
    assert_eq!(minimal.value_refs_named("Fn").unwrap(), 0);
    assert_eq!(minimal.build_excluded_count().unwrap(), None);
    assert!(
        phantom_fractal_analyze::impact(&minimal, 1, &ImpactOptions::default())
            .unwrap()
            .exact
    );

    // Go method triggers interface_dispatch even with trait_name == None.
    let mut g = target();
    g.facts_mut(1).kind = "method".to_owned();
    g.facts_mut(1).file = "pkg/a.go".to_owned();
    assert_eq!(reasons(&g), [(CompletenessReason::InterfaceDispatch, None)]);

    // ValueReference & BuildExcluded apply only when positive.
    let mut g2 = target();
    g2.value_refs.insert("t".to_owned(), 2);
    g2.build_excluded = Some(0);
    assert_eq!(
        reasons(&g2),
        [(CompletenessReason::ValueReference, Some(2))]
    );
    g2.build_excluded = Some(3);
    assert_eq!(
        reasons(&g2),
        [
            (CompletenessReason::ValueReference, Some(2)),
            (CompletenessReason::BuildExcluded, Some(3)),
        ]
    );

    // Go ExternalUsersPossible = false for package main and internal/... paths,
    // while normal Go libs and Rust internal/ paths stay true.
    for (file, pkg, target_name, lang, expected_external) in [
        ("pkg/a.go", "example.com/app", Some("pkg"), None, true),
        (
            "internal_helper/a.go",
            "example.com/app",
            Some("internal_helper"),
            None,
            true,
        ),
        ("internal/a.go", "example.com/app", Some("pkg"), None, false),
        (
            "pkg/internal/a.go",
            "example.com/app",
            Some("pkg"),
            None,
            false,
        ),
        (
            "cmd/main.go",
            "example.com/app/cmd/main",
            Some("main"),
            None,
            false,
        ),
        ("pkg/a.go", "main", Some("pkg"), None, true),
        (
            "custom/ext",
            "example.com/app",
            Some("pkg"),
            Some("go"),
            true,
        ),
        (
            "internal/custom.ext",
            "example.com/app",
            Some("pkg"),
            Some("go"),
            false,
        ),
        (
            "internal/a.rs",
            "rust_pkg",
            Some("rust_pkg"),
            Some("rust"),
            true,
        ),
    ] {
        let mut gx = target();
        gx.facts_mut(1).file = file.to_owned();
        gx.facts_mut(1).visibility = "public".to_owned();
        gx.facts_mut(1).package = Some(pkg.to_owned());
        if let Some(tname) = target_name {
            gx.target_names.insert(1, tname.to_owned());
        }
        gx.facts_mut(1).target_kind = Some("lib".to_owned());
        if let Some(l) = lang {
            gx.set_lang(file, l);
        }
        let got = reasons(&gx);
        let has_ext = got.iter().any(|(r, _)| *r == ExternalUsersPossible);
        assert_eq!(has_ext, expected_external, "{file} {pkg} {lang:?}: {got:?}");
    }

    // ManifestChanged fires on diff impact when DiffState::manifest_changed is true.
    let gm = target();
    let res = impact_many(
        &gm,
        &[],
        &ImpactOptions::default(),
        &DiffState {
            stale_index: false,
            unindexed_changes: 0,
            manifest_changed: true,
        },
    )
    .unwrap();
    assert!(!res.exact);
    assert_eq!(
        res.reasons
            .iter()
            .map(|r| r.reason.as_str())
            .collect::<Vec<_>>(),
        ["manifest_changed"]
    );
}

#[test]
fn store_impact_source_reads_go_lang_value_refs_and_source_skipped() {
    use phantom_fractal_analyze::{ImpactOptions, ImpactSource};
    use phantom_fractal_extract::lang::{
        Caller, Extract, ExtractError, FileModule, Item, ItemKind, Language, Method, Module,
        RefKind, Reference, Target, TargetKind, Visibility,
    };
    use phantom_fractal_store::{Package, Store};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            static C: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "phantom-analyze-store-{}-{}",
                std::process::id(),
                C.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p.join("graph.db"))
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            if let Some(p) = self.0.parent() {
                let _ = fs::remove_dir_all(p);
            }
        }
    }

    struct GoLang;
    impl Language for GoLang {
        fn name(&self) -> &'static str {
            "go"
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["go"]
        }
        fn qualifier_separator(&self) -> &'static str {
            "."
        }
        fn extract(&self, _s: &str) -> Result<Extract, ExtractError> {
            Ok(Extract {
                items: vec![
                    Item {
                        kind: ItemKind::Function,
                        name: "Hook".to_owned(),
                        visibility: Visibility::Public,
                        start_line: 1,
                        end_line: 1,
                        signature: "func Hook()".to_owned(),
                        doc_summary: None,
                        is_test: false,
                    },
                    Item {
                        kind: ItemKind::Struct,
                        name: "T".to_owned(),
                        visibility: Visibility::Public,
                        start_line: 2,
                        end_line: 2,
                        signature: "type T struct".to_owned(),
                        doc_summary: None,
                        is_test: false,
                    },
                ],
                item_scopes: vec![String::new(), String::new()],
                item_owners: vec![None, None],
                methods: vec![Method {
                    owner: "T".to_owned(),
                    owner_path: "T".to_owned(),
                    trait_name: None,
                    name: "Do".to_owned(),
                    visibility: Visibility::Public,
                    scope: String::new(),
                    start_line: 4,
                    end_line: 4,
                    signature: "func (T) Do()".to_owned(),
                    doc_summary: None,
                    is_test: false,
                }],
                refs: vec![Reference {
                    kind: RefKind::ValueRef,
                    text: "pkg.Hook".to_owned(),
                    last_segment: "Hook".to_owned(),
                    line: 5,
                    col: 1,
                    scope: String::new(),
                    from: Some(Caller {
                        name: "Do".to_owned(),
                        owner: Some("T".to_owned()),
                        start_line: 4,
                    }),
                    receiver: None,
                    receiver_type: None,
                    shadowed: false,
                }],
                ..Extract::default()
            })
        }
    }

    let tmp = Tmp::new();
    let mut store = Store::open(&tmp.0).unwrap();
    store
        .put_source("internal/pkg/a.go", "package pkg\n", 0, &GoLang)
        .unwrap();
    store
        .write_crate_graph_with_files(
            &[Package {
                name: "example.com/app".to_owned(),
                version: None,
                manifest_path: "go.mod".to_owned(),
            }],
            &[Target {
                id: 1,
                package: "example.com/app".to_owned(),
                name: "pkg".to_owned(),
                kind: TargetKind::Lib,
                root: String::new(),
            }],
            &[Module {
                id: 1,
                target: 1,
                parent: None,
                path: "example.com/app/internal/pkg".to_owned(),
                file: String::new(),
                scope: String::new(),
            }],
            &[FileModule {
                file: "internal/pkg/a.go".to_owned(),
                module: 1,
            }],
            &[],
        )
        .unwrap();
    store.set_workspace_fingerprint(&[0u8; 32], 2).unwrap();

    assert_eq!(store.rs_skipped().unwrap(), Some(2));
    assert_eq!(
        store.file_lang("internal/pkg/a.go").unwrap().as_deref(),
        Some("go")
    );
    assert_eq!(store.file_lang("src/lib.rs").unwrap(), None);
    assert_eq!(store.value_refs_named("Hook").unwrap(), 1);
    assert_eq!(store.value_refs_named("Other").unwrap(), 0);

    let hook_id = store.symbols_matching("Hook").unwrap()[0].id;
    let hook_imp =
        phantom_fractal_analyze::impact(&store, hook_id, &ImpactOptions::default()).unwrap();
    assert!(!hook_imp.exact);
    let r_hook: Vec<&str> = hook_imp.reasons.iter().map(|r| r.reason.as_str()).collect();
    assert!(r_hook.contains(&"value_reference"), "{r_hook:?}");
    assert!(r_hook.contains(&"parse_skipped_files"), "{r_hook:?}");
    assert!(
        !r_hook.contains(&"external_users_possible"),
        "internal/ must suppress external: {r_hook:?}"
    );

    let do_id = store.symbols_matching("Do").unwrap()[0].id;
    let do_imp = phantom_fractal_analyze::impact(&store, do_id, &ImpactOptions::default()).unwrap();
    let r_do: Vec<&str> = do_imp.reasons.iter().map(|r| r.reason.as_str()).collect();
    assert!(r_do.contains(&"interface_dispatch"), "{r_do:?}");
}

#[test]
fn every_completeness_reason_variant_is_reachable() {
    use phantom_fractal_analyze::{
        CompletenessReason, DiffState, ImpactOptions, impact, impact_many,
    };
    use std::collections::BTreeSet;

    let mut g = support::Graph::new();
    let id = g.function(1, "M");
    {
        let facts = g.facts_mut(id);
        facts.kind = "method".to_owned();
        facts.file = "src/a.go".to_owned();
        facts.visibility = "public".to_owned();
        facts.trait_name = Some("Tr".to_owned());
        facts.package = Some("pkg".to_owned());
    }
    let caller = g.function(2, "caller");
    g.call(caller, id, 1.0);
    g.langs.insert("src/a.go".to_owned(), "go".to_owned());
    g.unresolved.insert("M".to_owned(), 1);
    g.macro_args.insert("M".to_owned(), 1);
    g.value_refs.insert("M".to_owned(), 1);
    g.rs_skipped = Some(1);
    g.build_excluded = Some(2);

    let diff = DiffState {
        stale_index: true,
        unindexed_changes: 1,
        manifest_changed: true,
    };
    let a1 = impact(&g, id, &ImpactOptions::default()).unwrap();
    let a2 = impact_many(&g, &[id], &ImpactOptions::default(), &diff).unwrap();
    let seen: BTreeSet<&str> = a1
        .reasons
        .iter()
        .chain(a2.reasons.iter())
        .map(|r| r.reason.as_str())
        .collect();
    for variant in CompletenessReason::ALL {
        assert!(
            seen.contains(variant.as_str()),
            "unreachable CompletenessReason::{variant:?} in {seen:?}"
        );
    }
}
