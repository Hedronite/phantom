//! `Graph.test_modules`: every module whose declaration has `cfg_test`, and
//! every module below one (SPEC-w5 G1a).

use phantom_fractal_extract::graph::{Graph, Manifest, SourceFile, build};
use phantom_fractal_lang::{Language, ModDecl};
use phantom_fractal_lang_rust::RustLanguage;

fn decls(source: &str) -> Vec<ModDecl> {
    RustLanguage::new()
        .expect("query compiles")
        .extract(source)
        .expect("source parses")
        .mod_decls
}

fn file(path: &str, source: &str) -> SourceFile {
    SourceFile {
        path: path.to_owned(),
        mod_decls: decls(source),
    }
}

fn graph(files: &[SourceFile]) -> Graph {
    let manifest = Manifest {
        path: "Cargo.toml".to_owned(),
        text: "[package]\nname = \"sample\"\n".to_owned(),
    };
    build(&[manifest], files)
}

/// The module paths of `graph.test_modules`, in order.
fn test_paths(graph: &Graph) -> Vec<&str> {
    graph
        .test_modules
        .iter()
        .map(|id| {
            graph
                .modules
                .iter()
                .find(|module| module.id == *id)
                .map(|module| module.path.as_str())
                .expect("test module is a module")
        })
        .collect()
}

#[test]
fn cfg_test_modules_and_everything_below_them_are_test_modules() {
    let files = [
        file(
            "src/lib.rs",
            "mod a;\n#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod inline {\n    mod nested {}\n}\n\
             #[cfg(all(test, unix))]\nmod near;\nmod plain {}\n",
        ),
        file(
            "src/a.rs",
            "#[cfg(test)]\nmod inner {\n    mod deeper {\n        mod deepest;\n    }\n}\nmod b;\n",
        ),
        file("src/a/b.rs", ""),
        file("src/a/inner/deeper/deepest.rs", "mod leaf;\n"),
        file("src/a/inner/deeper/deepest/leaf.rs", ""),
        file("src/tests.rs", "mod sub;\nmod helper {}\n"),
        file("src/tests/sub.rs", ""),
        file("src/near.rs", ""),
    ];
    let graph = graph(&files);
    assert!(graph.orphans.is_empty(), "{:?}", graph.orphans);
    assert_eq!(
        test_paths(&graph),
        [
            "crate::tests",
            "crate::inline",
            "crate::inline::nested",
            "crate::a::inner",
            "crate::a::inner::deeper",
            "crate::a::inner::deeper::deepest",
            "crate::tests::sub",
            "crate::tests::helper",
            "crate::a::inner::deeper::deepest::leaf",
        ]
    );
}

#[test]
fn a_tree_without_cfg_test_has_no_test_modules() {
    let files = [
        file("src/lib.rs", "mod a;\n#[cfg(not(test))]\nmod b {}\n"),
        file("src/a.rs", "#[test]\nfn t() {}\n"),
    ];
    assert_eq!(test_paths(&graph(&files)), Vec::<&str>::new());
}

#[test]
fn a_test_module_in_one_target_does_not_mark_another() {
    let files = [
        file("src/lib.rs", "#[cfg(test)]\nmod tests {}\n"),
        file("src/main.rs", "mod tests {}\n"),
    ];
    let graph = graph(&files);
    let targets: Vec<_> = graph
        .test_modules
        .iter()
        .map(|id| graph.modules[usize::try_from(*id - 1).expect("id")].target)
        .collect();
    assert_eq!(test_paths(&graph), ["crate::tests"]);
    assert_eq!(targets.len(), 1);
    let lib = graph
        .targets
        .iter()
        .find(|t| t.root == "src/lib.rs")
        .expect("lib");
    assert_eq!(targets[0], lib.id);
}
