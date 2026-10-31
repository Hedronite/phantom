//! Workspace dependencies by line scan (SPEC-w4 G2a).

use phantom_fractal_extract::deps::scan;
use phantom_fractal_extract::graph::Manifest;
use phantom_fractal_lang::Dep;

const ROOT: &str = "[workspace]\nmembers = [\"core-lib\", \"app\", \"other\"]\n\n\
                    [workspace.dependencies]\n\
                    core-lib = { path = \"core-lib\", version = \"0.1\" }\n";

fn manifest(path: &str, text: &str) -> Manifest {
    Manifest {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

/// The root, `core-lib`, `other`, and `app` with `deps` as its
/// `[dependencies]` lines.
fn workspace(root: &str, deps: &str) -> Vec<Manifest> {
    vec![
        manifest("Cargo.toml", root),
        manifest("core-lib/Cargo.toml", "[package]\nname = \"core-lib\"\n"),
        manifest("other/Cargo.toml", "[package]\nname = \"other\"\n"),
        manifest(
            "app/Cargo.toml",
            &format!("[package]\nname = \"app\"\n\n[dependencies]\n{deps}\n"),
        ),
    ]
}

fn app_deps(deps: &str) -> Vec<Dep> {
    scan(&workspace(ROOT, deps))
}

fn on_core() -> Vec<Dep> {
    vec![Dep {
        package: "app".to_owned(),
        on: "core-lib".to_owned(),
        crate_name: "core_lib".to_owned(),
        dev: false,
    }]
}

#[test]
fn a_single_line_path_dependency_on_a_workspace_package_counts() {
    for line in [
        "core-lib = { path = \"../core-lib\" }",
        "core-lib = { path = '../core-lib' }",
        "core-lib = { version = \"0.1\", path = \"../core-lib\", features = [\"a\", \"b\"] }",
        "core-lib = { path = \"../core-lib/\", optional = true } # why",
        "core-lib={path=\"./../x/../core-lib\"}",
    ] {
        assert_eq!(app_deps(line), on_core(), "{line}");
    }
}

#[test]
fn a_workspace_dependency_counts_when_the_root_gives_its_path() {
    for line in [
        "core-lib.workspace = true",
        "core-lib = { workspace = true }",
        "core-lib = { workspace = true, features = [\"x\"], optional = true }",
    ] {
        assert_eq!(app_deps(line), on_core(), "{line}");
    }
}

#[test]
fn other_dependency_forms_give_no_dependency() {
    for line in [
        // A version-only line, and a version-only table.
        "core-lib = \"0.1\"",
        "core-lib = { version = \"0.1\" }",
        // A rename, a git source, and a quoted key.
        "core = { path = \"../core-lib\", package = \"core-lib\" }",
        "core-lib = { path = \"../core-lib\", package = \"core-lib\" }",
        "core-lib = { git = \"https://example.invalid/core\", path = \"../core-lib\" }",
        "\"core-lib\" = { path = \"../core-lib\" }",
        // An inline table over several lines.
        "core-lib = {\n    path = \"../core-lib\",\n}",
        // `workspace` and `path` together, and `workspace = false`.
        "core-lib = { workspace = true, path = \"../core-lib\" }",
        "core-lib = { workspace = false }",
        "core-lib.workspace = false",
        // An open string or bracket.
        "core-lib = { path = \"../core-lib, features = [\"a\" }",
        "core-lib = { path = \"../core-lib\", features = [\"a\" }",
        // Paths outside the root, absolute, or to the wrong directory.
        "core-lib = { path = \"../../core-lib\" }",
        "core-lib = { path = \"/core-lib\" }",
        "core-lib = { path = \"/../core-lib\" }",
        "core-lib = { path = \"..\\\\core-lib\" }",
        "core-lib = { path = \"../other\" }",
        "other = { path = \"../core-lib\" }",
        "missing = { path = \"../missing\" }",
        // The package itself.
        "app = { path = \".\" }",
    ] {
        assert_eq!(app_deps(line), [], "{line}");
    }
}

#[test]
fn only_the_dependencies_and_dev_dependencies_tables_count() {
    for section in [
        "[dependencies.core-lib]\npath = \"../core-lib\"",
        "[dev-dependencies.core-lib]\npath = \"../core-lib\"",
        "[build-dependencies]\ncore-lib = { path = \"../core-lib\" }",
        "[target.'cfg(unix)'.dependencies]\ncore-lib = { path = \"../core-lib\" }",
        "[target.'cfg(unix)'.dev-dependencies]\ncore-lib = { path = \"../core-lib\" }",
        "[workspace.dependencies]\ncore-lib = { path = \"../core-lib\" }",
    ] {
        let manifests = workspace(ROOT, &format!("serde = \"1\"\n\n{section}"));
        assert_eq!(scan(&manifests), [], "{section}");
    }
}

#[test]
fn a_workspace_dependency_needs_a_single_line_root_path_entry() {
    for root in [
        // No entry, a version-only entry, and a multi-line entry.
        "[workspace]\n",
        "[workspace.dependencies]\ncore-lib = \"0.1\"\n",
        "[workspace.dependencies]\ncore-lib = {\n  path = \"core-lib\"\n}\n",
        // A rename, a path outside the root, and the wrong package.
        "[workspace.dependencies]\ncore-lib = { path = \"core-lib\", package = \"x\" }\n",
        "[workspace.dependencies]\ncore-lib = { path = \"../core-lib\" }\n",
        "[workspace.dependencies]\ncore-lib = { path = \"other\" }\n",
        // The entry under another table.
        "[dependencies]\ncore-lib = { path = \"core-lib\" }\n",
    ] {
        let manifests = workspace(root, "core-lib.workspace = true");
        assert_eq!(scan(&manifests), [], "{root}");
    }
}

#[test]
fn only_the_root_manifest_gives_workspace_paths() {
    let mut manifests = workspace("[workspace]\n", "core-lib.workspace = true");
    manifests.push(manifest(
        "nested/Cargo.toml",
        "[workspace.dependencies]\ncore-lib = { path = \"core-lib\" }\n",
    ));
    assert_eq!(scan(&manifests), []);
}

#[test]
fn crate_names_turn_dashes_to_underscores_and_repeat_once() {
    let manifests = vec![
        manifest("Cargo.toml", "[workspace]\n"),
        manifest("a/Cargo.toml", "[package]\nname = \"my-util-lib\"\n"),
        manifest("b/Cargo.toml", "[package]\nname = \"plain\"\n"),
        manifest(
            "app/Cargo.toml",
            "[package]\nname = \"app\"\n[dependencies]\n\
             my-util-lib = { path = \"../a\" }\nplain = { path = \"../b\" }\n\
             plain = { path = \"../b\" }\n",
        ),
    ];
    let deps = scan(&manifests);
    let got: Vec<(&str, &str, &str)> = deps
        .iter()
        .map(|d| (d.package.as_str(), d.on.as_str(), d.crate_name.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("app", "my-util-lib", "my_util_lib"),
            ("app", "plain", "plain")
        ]
    );
}

#[test]
fn a_root_package_can_be_depended_on_and_can_depend() {
    let manifests = vec![
        manifest(
            "Cargo.toml",
            "[package]\nname = \"top\"\n[dependencies]\nleaf = { path = \"leaf\" }\n",
        ),
        manifest(
            "leaf/Cargo.toml",
            "[package]\nname = \"leaf\"\n[dependencies]\ntop = { path = \"..\" }\n",
        ),
    ];
    let got: Vec<(String, String)> = scan(&manifests)
        .into_iter()
        .map(|d| (d.package, d.on))
        .collect();
    assert_eq!(
        got,
        [
            ("top".to_owned(), "leaf".to_owned()),
            ("leaf".to_owned(), "top".to_owned())
        ]
    );
}

/// `app` with `deps` as its `[dev-dependencies]` lines.
fn app_dev_deps(deps: &str) -> Vec<Dep> {
    scan(&workspace(
        ROOT,
        &format!("serde = \"1\"\n\n[dev-dependencies]\n{deps}"),
    ))
}

fn dev_on_core() -> Vec<Dep> {
    vec![Dep {
        dev: true,
        ..on_core().remove(0)
    }]
}

#[test]
fn a_dev_dependency_counts_in_the_same_forms_marked_dev() {
    for line in [
        "core-lib = { path = \"../core-lib\" }",
        "core-lib = { version = \"0.1\", path = '../core-lib', features = [\"a\"] }",
        "core-lib.workspace = true",
        "core-lib = { workspace = true, optional = true }",
    ] {
        assert_eq!(app_dev_deps(line), dev_on_core(), "{line}");
    }
    for line in [
        "core-lib = \"0.1\"",
        "core = { path = \"../core-lib\", package = \"core-lib\" }",
        "core-lib = { git = \"https://example.invalid/core\", path = \"../core-lib\" }",
        "\"core-lib\" = { path = \"../core-lib\" }",
        "core-lib = {\n    path = \"../core-lib\",\n}",
        "core-lib = { path = \"/abs/core-lib\" }",
        "other = { path = \"../core-lib\" }",
        "app = { path = \".\" }",
    ] {
        assert_eq!(app_dev_deps(line), [], "{line}");
    }
}

#[test]
fn a_package_both_tables_name_gives_two_entries() {
    let line = "core-lib = { path = \"../core-lib\" }";
    let manifests = workspace(ROOT, &format!("{line}\n\n[dev-dependencies]\n{line}"));
    let mut want = on_core();
    want.extend(dev_on_core());
    assert_eq!(scan(&manifests), want);
}
