//! Packages, targets, and the module tree (SPEC-w3 G2a).

use phantom_fractal_extract::graph::{Graph, Manifest, SourceFile, build};
use phantom_fractal_lang::{ModDecl, TargetKind};

fn manifest(path: &str, text: &str) -> Manifest {
    Manifest {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

fn file(path: &str, decls: Vec<ModDecl>) -> SourceFile {
    SourceFile {
        path: path.to_owned(),
        mod_decls: decls,
    }
}

fn decl(name: &str, scope: &str, inline: bool, path: Option<&str>) -> ModDecl {
    ModDecl {
        name: name.to_owned(),
        scope: scope.to_owned(),
        inline,
        path: path.map(str::to_owned),
        line: 1,
    }
}

fn package(name: &str) -> Manifest {
    manifest("Cargo.toml", &format!("[package]\nname = \"{name}\"\n"))
}

/// `(target root, module path, file, scope)` for every module row.
fn modules(graph: &Graph) -> Vec<(String, String, String, String)> {
    graph
        .modules
        .iter()
        .map(|m| {
            let target = graph
                .targets
                .iter()
                .find(|t| t.id == m.target)
                .expect("target");
            (
                target.root.clone(),
                m.path.clone(),
                m.file.clone(),
                m.scope.clone(),
            )
        })
        .collect()
}

fn row(root: &str, path: &str, file: &str, scope: &str) -> (String, String, String, String) {
    (
        root.to_owned(),
        path.to_owned(),
        file.to_owned(),
        scope.to_owned(),
    )
}

#[test]
fn packages_come_from_a_package_table_by_line_scan() {
    let manifests = [
        manifest("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n"),
        manifest(
            "crates/a/Cargo.toml",
            "[dependencies]\nname = \"not-this\"\n\n[package]\nname = \"alpha\" # the name\nversion = \"1.2.3\"\n",
        ),
        manifest(
            "crates/b/Cargo.toml",
            "[package]\nversion.workspace = true\nnamespace = \"x\"\n",
        ),
        manifest("crates/c/Cargo.toml", "[package]\nname = 'gamma'\n"),
        manifest("crates/d/Cargo.toml", "[package]\nname = \"alpha\"\n"),
    ];
    let graph = build(&manifests, &[]);
    let got: Vec<(&str, Option<&str>, &str)> = graph
        .packages
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.version.as_deref(),
                p.manifest_path.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("alpha", Some("1.2.3"), "crates/a/Cargo.toml"),
            ("b", None, "crates/b/Cargo.toml"),
            ("gamma", None, "crates/c/Cargo.toml"),
        ]
    );
}

#[test]
fn targets_are_lib_main_and_each_src_bin_file() {
    let files = [
        file("src/lib.rs", vec![]),
        file("src/main.rs", vec![]),
        file("src/bin/tool.rs", vec![]),
        file("src/bin/nested/deep.rs", vec![]),
        file("other/src/lib.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    let got: Vec<(TargetKind, &str, &str)> = graph
        .targets
        .iter()
        .map(|t| (t.kind, t.name.as_str(), t.root.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (TargetKind::Lib, "pkg", "src/lib.rs"),
            (TargetKind::Bin, "pkg", "src/main.rs"),
            (TargetKind::Bin, "tool", "src/bin/tool.rs"),
        ]
    );
    assert!(graph.targets.iter().all(|t| t.package == "pkg"));
}

#[test]
fn a_package_in_a_subdirectory_has_targets_under_it() {
    let manifests = [manifest("crates/a/Cargo.toml", "[package]\nname = \"a\"\n")];
    let files = [
        file("crates/a/src/lib.rs", vec![]),
        file("src/lib.rs", vec![]),
    ];
    let graph = build(&manifests, &files);
    let roots: Vec<&str> = graph.targets.iter().map(|t| t.root.as_str()).collect();
    assert_eq!(roots, ["crates/a/src/lib.rs"]);
    assert_eq!(graph.orphans, ["src/lib.rs"]);
}

#[test]
fn module_files_follow_the_2018_rules() {
    let files = [
        file(
            "src/lib.rs",
            vec![
                decl("a", "", false, None),
                decl("b", "", false, None),
                decl("c", "", true, None),
                decl("d", "c", false, None),
                decl("p", "", false, Some("other/p.rs")),
                decl("ghost", "", false, None),
            ],
        ),
        file("src/a.rs", vec![decl("e", "", false, None)]),
        file("src/a/e.rs", vec![]),
        file("src/e.rs", vec![]),
        file("src/b/mod.rs", vec![decl("f", "", false, None)]),
        file("src/b/f.rs", vec![]),
        file("src/c/d.rs", vec![]),
        file("src/other/p.rs", vec![decl("q", "", false, None)]),
        file("src/other/q.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    let lib = "src/lib.rs";
    assert_eq!(
        modules(&graph),
        [
            row(lib, "crate", "src/lib.rs", ""),
            row(lib, "crate::a", "src/a.rs", ""),
            row(lib, "crate::b", "src/b/mod.rs", ""),
            row(lib, "crate::c", "src/lib.rs", "c"),
            row(lib, "crate::c::d", "src/c/d.rs", ""),
            row(lib, "crate::p", "src/other/p.rs", ""),
            row(lib, "crate::a::e", "src/a/e.rs", ""),
            row(lib, "crate::b::f", "src/b/f.rs", ""),
            row(lib, "crate::p::q", "src/other/q.rs", ""),
        ]
    );
    // `src/e.rs` is beside `a.rs`, not in `a/`, so no target reaches it.
    assert_eq!(graph.orphans, ["src/e.rs"]);
    let parents: Vec<Option<i64>> = graph.modules.iter().map(|m| m.parent).collect();
    assert_eq!(
        parents,
        [
            None,
            Some(1),
            Some(1),
            Some(1),
            Some(4),
            Some(1),
            Some(2),
            Some(3),
            Some(6)
        ]
    );
}

#[test]
fn x_rs_wins_over_x_mod_rs() {
    let files = [
        file("src/lib.rs", vec![decl("x", "", false, None)]),
        file("src/x.rs", vec![]),
        file("src/x/mod.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    assert_eq!(graph.modules[1].file, "src/x.rs");
    assert_eq!(graph.orphans, ["src/x/mod.rs"]);
}

#[test]
fn a_path_attribute_must_stay_inside_the_root_and_name_a_kept_file() {
    let files = [
        file(
            "src/lib.rs",
            vec![
                decl("up", "", false, Some("../shared.rs")),
                decl("out", "", false, Some("../../outside.rs")),
                decl("abs", "", false, Some("/etc/passwd")),
                decl("missing", "", false, Some("nowhere.rs")),
                decl("dotted", "", false, Some("./x/../y.rs")),
            ],
        ),
        file("shared.rs", vec![]),
        file("src/y.rs", vec![]),
        // Climbing past the root must not fold back onto this kept file.
        file("outside.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    let got: Vec<(&str, &str)> = graph
        .modules
        .iter()
        .map(|m| (m.path.as_str(), m.file.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("crate", "src/lib.rs"),
            ("crate::up", "shared.rs"),
            ("crate::dotted", "src/y.rs"),
        ]
    );
    assert_eq!(graph.orphans, ["outside.rs"]);
}

#[test]
fn a_path_inside_an_inline_module_is_relative_to_its_directory() {
    let files = [
        file(
            "src/a.rs",
            vec![
                decl("inner", "", true, None),
                decl("z", "inner", false, Some("z.rs")),
            ],
        ),
        file("src/lib.rs", vec![decl("a", "", false, None)]),
        file("src/a/inner/z.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    let got: Vec<&str> = graph.modules.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(
        got,
        ["crate", "crate::a", "crate::a::inner", "crate::a::inner::z"]
    );
    assert_eq!(graph.modules[3].file, "src/a/inner/z.rs");
}

#[test]
fn a_file_reached_twice_keeps_its_first_module() {
    let files = [
        file("src/lib.rs", vec![decl("shared", "", false, None)]),
        file("src/main.rs", vec![decl("shared", "", false, None)]),
        file("src/shared.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    assert_eq!(
        modules(&graph),
        [
            row("src/lib.rs", "crate", "src/lib.rs", ""),
            row("src/lib.rs", "crate::shared", "src/shared.rs", ""),
            row("src/main.rs", "crate", "src/main.rs", ""),
        ]
    );
}

#[test]
fn files_no_target_reaches_are_orphans() {
    let files = [
        file("src/lib.rs", vec![]),
        file("src/lost.rs", vec![]),
        file("tests/it.rs", vec![]),
    ];
    let graph = build(&[package("pkg")], &files);
    assert_eq!(graph.orphans, ["src/lost.rs", "tests/it.rs"]);
    let none = build(&[], &files);
    assert!(none.targets.is_empty() && none.modules.is_empty());
    assert_eq!(none.orphans.len(), 3);
}
