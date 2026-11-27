//! Combined extract + store + analyze + diff multilang stub tests (G2b).

#[test]
fn store_impact_source_reads_go_lang_value_refs_and_source_skipped() {
    use phantom_fractal_analyze::{ImpactOptions, ImpactSource};
    use phantom_fractal_lang::{
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
        .put_source(
            "internal/pkg/a.custom",
            "package pkg
",
            0,
            &GoLang,
        )
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
                file: "internal/pkg/a.custom".to_owned(),
                module: 1,
            }],
            &[],
        )
        .unwrap();
    store.set_workspace_fingerprint(&[0u8; 32], 0).unwrap();

    assert_eq!(
        store.file_lang("internal/pkg/a.custom").unwrap().as_deref(),
        Some("go")
    );
    assert_eq!(store.file_lang("missing.rs").unwrap(), None);
    assert_eq!(store.value_refs_named("Hook").unwrap(), 1);
    assert_eq!(store.value_refs_named("Other").unwrap(), 0);

    let hook_id = store.symbols_matching("Hook").unwrap()[0].id;
    let hook_imp =
        phantom_fractal_analyze::impact(&store, hook_id, &ImpactOptions::default()).unwrap();
    assert!(!hook_imp.exact);
    let r_hook: Vec<&str> = hook_imp.reasons.iter().map(|r| r.reason.as_str()).collect();
    assert!(r_hook.contains(&"value_reference"), "{r_hook:?}");
    assert!(
        !r_hook.contains(&"external_users_possible"),
        "internal/ must suppress external: {r_hook:?}"
    );

    let do_id = store.symbols_matching("Do").unwrap()[0].id;
    let do_imp = phantom_fractal_analyze::impact(&store, do_id, &ImpactOptions::default()).unwrap();
    let r_do: Vec<&str> = do_imp.reasons.iter().map(|r| r.reason.as_str()).collect();
    assert!(r_do.contains(&"interface_dispatch"), "{r_do:?}");
}
