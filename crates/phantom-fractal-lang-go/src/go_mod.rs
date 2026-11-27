//! Walk-only Go module (`go.mod`, `go.work`) and package graph builder.
//!
//! Strictly operates on caller-provided `(path, content)` pairs and never
//! touches the filesystem.

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::{Node, Parser};

use crate::build_tags::{BuildEnv, DEFAULT_GO_MINOR, is_test_filename, matches_file};

/// One repository-relative walked source or manifest file in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoSourceFile<'a> {
    /// Repo-relative `/`-separated path (e.g. `"internal/core/core.go"`).
    pub path: &'a str,
    /// UTF-8 contents of the file.
    pub content: &'a str,
}

/// Metadata parsed from a walked `go.mod` manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoModuleSpec {
    /// Directory holding `go.mod` (`""` for repository root).
    pub dir: String,
    /// Declared module import prefix (e.g. `"example.com/battery"`).
    pub module_path: String,
    /// Declared `go 1.N` minor version, or [`DEFAULT_GO_MINOR`].
    pub go_minor: u32,
    /// Whether this module belongs to the active workspace graph.
    pub in_workspace: bool,
}

/// One resolved Go package built from build-included `.go` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoPackage {
    /// Canonical Go import path (external `_test` tests append `_test`).
    pub import_path: String,
    /// Declared `package <name>` identifier.
    pub package_name: String,
    /// Enclosing module path (`""` when no `go.mod` encloses the file).
    pub module_path: String,
    /// Enclosing `go.mod` directory (`""` for root or rootless files).
    pub module_dir: String,
    /// Package directory relative to repo root (`""` for root).
    pub dir: String,
    /// True for an external `package foo_test` test package.
    pub is_external_test: bool,
    /// True when the package declares `package main`.
    pub is_main: bool,
    /// Included repo-relative `.go` file paths, sorted.
    pub files: Vec<String>,
}

/// Why a walked `.go` file was kept out of [`PackageGraph::packages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludeReason {
    /// Inside `testdata/`, `vendor/`, `.`-prefixed, or `_`-prefixed path.
    IgnoredPath,
    /// Excluded by pinned `linux/amd64` filename suffixes or build tags.
    BuildConstraint,
    /// Owned by a nested `go.mod` outside any `go.work` `use` list.
    NestedModule,
    /// Missing a valid `package <name>` clause.
    MissingPackageClause,
}

/// One walked `.go` file excluded from the active package graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedGoFile {
    pub path: String,
    pub reason: ExcludeReason,
}

/// Walk-built Go modules, active packages, file memberships, and exclusions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageGraph {
    pub modules: Vec<GoModuleSpec>,
    pub packages: Vec<GoPackage>,
    pub file_to_package: BTreeMap<String, String>,
    pub excluded: Vec<ExcludedGoFile>,
}

/// Builds a [`PackageGraph`] from walked manifests and `.go` files only.
pub fn build_package_graph(files: &[GoSourceFile<'_>]) -> PackageGraph {
    let mut modules = collect_modules(files);
    apply_workspace_membership(files, &mut modules);

    let mut graph = PackageGraph::default();
    let mut pkg_map: BTreeMap<(String, String), GoPackage> = BTreeMap::new();

    for file in files {
        if file.path.ends_with(".go") {
            classify_go_file(file, &modules, &mut pkg_map, &mut graph);
        }
    }

    graph.modules = modules.into_values().collect();
    graph.packages = pkg_map.into_values().collect();
    graph
}

fn classify_go_file(
    file: &GoSourceFile<'_>,
    modules: &BTreeMap<String, GoModuleSpec>,
    pkg_map: &mut BTreeMap<(String, String), GoPackage>,
    graph: &mut PackageGraph,
) {
    if is_ignored_go_path(file.path) {
        return push_excluded(graph, file.path, ExcludeReason::IgnoredPath);
    }
    let owner = find_enclosing_module(parent_dir(file.path), modules);
    if owner.is_some_and(|spec| !spec.in_workspace) {
        return push_excluded(graph, file.path, ExcludeReason::NestedModule);
    }
    let go_minor = owner.map_or(DEFAULT_GO_MINOR, |m| m.go_minor);
    if !matches_file(file.path, file.content, &BuildEnv::with_go_minor(go_minor)) {
        return push_excluded(graph, file.path, ExcludeReason::BuildConstraint);
    }
    let Some(pkg_name) = parse_package_clause(file.content) else {
        return push_excluded(graph, file.path, ExcludeReason::MissingPackageClause);
    };
    add_included_file(file.path, &pkg_name, owner, pkg_map, graph);
}

fn push_excluded(graph: &mut PackageGraph, path: &str, reason: ExcludeReason) {
    graph.excluded.push(ExcludedGoFile {
        path: path.to_owned(),
        reason,
    });
}

fn add_included_file(
    path: &str,
    pkg_name: &str,
    owner: Option<&GoModuleSpec>,
    pkg_map: &mut BTreeMap<(String, String), GoPackage>,
    graph: &mut PackageGraph,
) {
    let dir = parent_dir(path).to_owned();
    let module_dir = owner.map_or_else(String::new, |m| m.dir.clone());
    let module_path = owner.map_or_else(String::new, |m| m.module_path.clone());
    let rel = relative_dir(&dir, &module_dir);
    let base_import = join_import_path(&module_path, rel, pkg_name);
    let is_external_test = is_test_filename(path) && pkg_name.ends_with("_test");
    let import_path = if is_external_test {
        format!("{base_import}_test")
    } else {
        base_import
    };
    graph
        .file_to_package
        .insert(path.to_owned(), import_path.clone());
    let entry = pkg_map
        .entry((import_path.clone(), pkg_name.to_owned()))
        .or_insert_with(|| GoPackage {
            import_path,
            package_name: pkg_name.to_owned(),
            module_path,
            module_dir,
            dir,
            is_external_test,
            is_main: pkg_name == "main",
            files: Vec::new(),
        });
    entry.files.push(path.to_owned());
    entry.files.sort();
}

/// Returns true when a repo-relative path hits Go ignored dirs or files.
pub fn is_ignored_go_path(path: &str) -> bool {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    if file.starts_with('.') || file.starts_with('_') {
        return true;
    }
    dir.split('/').filter(|seg| !seg.is_empty()).any(|seg| {
        seg.starts_with('.') || seg.starts_with('_') || seg == "testdata" || seg == "vendor"
    })
}

/// Parses `module` and `go 1.N` from `go.mod` text.
pub fn parse_go_mod(content: &str) -> (String, u32) {
    let mut module_path = String::new();
    let mut go_minor = DEFAULT_GO_MINOR;
    for raw in content.lines() {
        let line = strip_go_comment(raw).trim();
        if let Some(rest) = line.strip_prefix("module").and_then(strip_leading_ws) {
            module_path = unquote_token(rest);
        } else if let Some(rest) = line.strip_prefix("go").and_then(strip_leading_ws)
            && let Some(minor) = parse_go_version_minor(rest)
        {
            go_minor = minor;
        }
    }
    (module_path, go_minor)
}

/// Parses `use` directory entries from `go.work` text.
pub fn parse_go_work_uses(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for raw in content.lines() {
        let line = strip_go_comment(raw).trim();
        if !line.is_empty() {
            step_go_work_line(line, &mut in_block, &mut out);
        }
    }
    out
}

fn step_go_work_line(line: &str, in_block: &mut bool, out: &mut Vec<String>) {
    if *in_block {
        if line == ")" {
            *in_block = false;
        } else {
            push_unquoted_nonempty(line, out);
        }
        return;
    }
    let Some(rest) = line.strip_prefix("use").map(str::trim_start) else {
        return;
    };
    if rest == "(" {
        *in_block = true;
    } else {
        push_unquoted_nonempty(rest, out);
    }
}

/// Resolves `rel` lexically beneath `base_dir`, returning `None` on root escape or absolute paths.
pub fn lexically_join_walked(base_dir: &str, rel: &str) -> Option<String> {
    if rel.starts_with('/') {
        return None;
    }
    let mut stack: Vec<&str> = base_dir
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    for seg in rel.split('/') {
        apply_path_segment(&mut stack, seg)?;
    }
    Some(stack.join("/"))
}

fn apply_path_segment<'a>(stack: &mut Vec<&'a str>, seg: &'a str) -> Option<()> {
    match seg {
        "" | "." => Some(()),
        ".." => stack.pop().map(|_| ()),
        other => {
            stack.push(other);
            Some(())
        }
    }
}

/// Parses the declared `package <name>` identifier from Go source text.
pub fn parse_package_clause(source: &str) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();
    let src = source.as_bytes();
    let mut cursor = root.walk();
    root.children(&mut cursor)
        .find(|c| c.kind() == "package_clause")
        .and_then(|clause| package_ident_text(clause, src))
}

fn package_ident_text(clause: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = clause.walk();
    for child in clause.children(&mut cursor) {
        if child.kind() == "package_identifier" {
            let text = child.utf8_text(src).ok()?.trim();
            if !text.is_empty() {
                return Some(text.to_owned());
            }
        }
    }
    None
}

fn collect_modules(files: &[GoSourceFile<'_>]) -> BTreeMap<String, GoModuleSpec> {
    let mut out = BTreeMap::new();
    for file in files {
        let name = file.path.rsplit('/').next().unwrap_or(file.path);
        if name != "go.mod" || is_ignored_go_path(file.path) {
            continue;
        }
        let dir = parent_dir(file.path).to_owned();
        let (module_path, go_minor) = parse_go_mod(file.content);
        out.insert(
            dir.clone(),
            GoModuleSpec {
                dir,
                module_path,
                go_minor,
                in_workspace: false,
            },
        );
    }
    out
}

fn apply_workspace_membership(
    files: &[GoSourceFile<'_>],
    modules: &mut BTreeMap<String, GoModuleSpec>,
) {
    if let Some(claimed) = collect_go_work_dirs(files) {
        for (dir, spec) in modules.iter_mut() {
            spec.in_workspace = claimed.contains(dir);
        }
        return;
    }
    let all_dirs: Vec<String> = modules.keys().cloned().collect();
    for (dir, spec) in modules.iter_mut() {
        spec.in_workspace = !has_strict_ancestor(dir, &all_dirs);
    }
}

fn collect_go_work_dirs(files: &[GoSourceFile<'_>]) -> Option<BTreeSet<String>> {
    let mut found_work = false;
    let mut claimed = BTreeSet::new();
    for file in files {
        let name = file.path.rsplit('/').next().unwrap_or(file.path);
        if name != "go.work" || is_ignored_go_path(file.path) {
            continue;
        }
        found_work = true;
        let work_dir = parent_dir(file.path);
        for use_entry in parse_go_work_uses(file.content) {
            if let Some(joined) = lexically_join_walked(work_dir, &use_entry) {
                claimed.insert(joined);
            }
        }
    }
    found_work.then_some(claimed)
}

fn has_strict_ancestor(dir: &str, all_dirs: &[String]) -> bool {
    all_dirs.iter().any(|cand| is_strict_ancestor(cand, dir))
}

fn is_strict_ancestor(ancestor: &str, child: &str) -> bool {
    if ancestor == child {
        return false;
    }
    ancestor.is_empty()
        || child
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn find_enclosing_module<'m>(
    dir: &str,
    modules: &'m BTreeMap<String, GoModuleSpec>,
) -> Option<&'m GoModuleSpec> {
    // Reverse BTreeMap order guarantees deeper nested module dirs visit before ancestors.
    modules
        .iter()
        .rev()
        .find(|(mod_dir, _)| dir == mod_dir.as_str() || is_strict_ancestor(mod_dir, dir))
        .map(|(_, spec)| spec)
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn relative_dir<'d>(dir: &'d str, module_dir: &str) -> &'d str {
    if module_dir.is_empty() {
        return dir;
    }
    dir.strip_prefix(module_dir)
        .map_or(dir, |rest| rest.strip_prefix('/').unwrap_or(rest))
}

fn join_import_path(module_path: &str, rel_dir: &str, pkg_name: &str) -> String {
    match (module_path.is_empty(), rel_dir.is_empty()) {
        (false, true) => module_path.to_owned(),
        (false, false) => format!("{module_path}/{rel_dir}"),
        (true, false) => rel_dir.to_owned(),
        (true, true) => pkg_name.to_owned(),
    }
}

fn strip_go_comment(raw: &str) -> &str {
    raw.split_once("//").map_or(raw, |(head, _)| head)
}

fn strip_leading_ws(s: &str) -> Option<&str> {
    let trimmed = s.trim_start();
    (trimmed.len() < s.len()).then_some(trimmed)
}

fn unquote_token(rest: &str) -> String {
    rest.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_owned()
}

fn push_unquoted_nonempty(line: &str, out: &mut Vec<String>) {
    let clean = unquote_token(line);
    if !clean.is_empty() {
        out.push(clean);
    }
}

fn parse_go_version_minor(rest: &str) -> Option<u32> {
    let ver = rest.split_whitespace().next()?.strip_prefix("1.")?;
    ver.split('.')
        .next()?
        .parse::<u32>()
        .ok()
        .filter(|&n| n >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_packages_with_external_test_nested_isolation_and_ignores() {
        let files = [
            GoSourceFile {
                path: "go.mod",
                content: "module example.com/battery // comment\ngo 1.24.0\n",
            },
            GoSourceFile {
                path: "nested/go.mod",
                content: "module example.com/nested\ngo 1.26\n",
            },
            GoSourceFile {
                path: "nested/n.go",
                content: "package nested\nfunc N() int { return 1 }\n",
            },
            GoSourceFile {
                path: "lib/lib.go",
                content: "package lib\nfunc Direct() {}\n",
            },
            GoSourceFile {
                path: "lib/lib_test.go",
                content: "package lib\nfunc TestInternal() {}\n",
            },
            GoSourceFile {
                path: "lib/ext_test.go",
                content: "package lib_test\nfunc TestExternal() {}\n",
            },
            GoSourceFile {
                path: "lib/os_darwin.go",
                content: "package lib\nfunc openFile() {}\n",
            },
            GoSourceFile {
                path: "lib/tagged.go",
                content: "//go:build special\n\npackage lib\nfunc Special() {}\n",
            },
            GoSourceFile {
                path: "testdata/x/x.go",
                content: "package x\nfunc T() {}\n",
            },
            GoSourceFile {
                path: "vendor/v/v.go",
                content: "package v\n",
            },
            GoSourceFile {
                path: "lib/_gen.go",
                content: "package lib\n",
            },
            GoSourceFile {
                path: "lib/.dot.go",
                content: "package lib\n",
            },
            GoSourceFile {
                path: ".git/hooks.go",
                content: "package hooks\n",
            },
            GoSourceFile {
                path: "_build/gen.go",
                content: "package build\n",
            },
            GoSourceFile {
                path: "cmd/app/main.go",
                content: "package main\nfunc main() {}\n",
            },
            GoSourceFile {
                path: "broken/nopkg.go",
                content: "// no package clause\n",
            },
        ];

        let graph = build_package_graph(&files);
        assert_eq!(graph.modules.len(), 2);
        assert!(
            graph
                .modules
                .iter()
                .any(|m| m.dir.is_empty() && m.in_workspace && m.go_minor == 24)
        );
        assert!(
            graph
                .modules
                .iter()
                .any(|m| m.dir == "nested" && !m.in_workspace)
        );
        assert_eq!(
            graph.file_to_package.get("lib/lib.go").map(String::as_str),
            Some("example.com/battery/lib")
        );
        assert_eq!(
            graph
                .file_to_package
                .get("lib/ext_test.go")
                .map(String::as_str),
            Some("example.com/battery/lib_test")
        );
        assert!(!graph.file_to_package.contains_key("nested/n.go"));
        let cmd_pkg = graph
            .packages
            .iter()
            .find(|p| p.package_name == "main")
            .unwrap();
        assert!(cmd_pkg.is_main);
        let lib_pkg = graph
            .packages
            .iter()
            .find(|p| p.package_name == "lib")
            .unwrap();
        assert!(!lib_pkg.is_main);

        let reasons: BTreeMap<&str, ExcludeReason> = graph
            .excluded
            .iter()
            .map(|e| (e.path.as_str(), e.reason))
            .collect();
        assert_eq!(
            reasons.get("nested/n.go"),
            Some(&ExcludeReason::NestedModule)
        );
        assert_eq!(
            reasons.get("lib/os_darwin.go"),
            Some(&ExcludeReason::BuildConstraint)
        );
        assert_eq!(
            reasons.get("testdata/x/x.go"),
            Some(&ExcludeReason::IgnoredPath)
        );
        assert_eq!(
            reasons.get("lib/.dot.go"),
            Some(&ExcludeReason::IgnoredPath)
        );
        assert_eq!(
            reasons.get(".git/hooks.go"),
            Some(&ExcludeReason::IgnoredPath)
        );
        assert_eq!(
            reasons.get("_build/gen.go"),
            Some(&ExcludeReason::IgnoredPath)
        );
        assert_eq!(
            reasons.get("broken/nopkg.go"),
            Some(&ExcludeReason::MissingPackageClause)
        );
    }

    #[test]
    fn go_work_claims_walked_modules_and_ignores_outside_escapes() {
        let files = [
            GoSourceFile {
                path: "go.work",
                content: "\ngo 1.25\nuse (\n  ./a\n  \"../outside\"\n  /etc/passwd\n)\nuse ./b\n",
            },
            GoSourceFile {
                path: "a/go.mod",
                content: "module \"example.com/a\"\n",
            },
            GoSourceFile {
                path: "a/a.go",
                content: "package a\nfunc A() {}\n",
            },
            GoSourceFile {
                path: "b/go.mod",
                content: "module example.com/b\ngo 1.22\n",
            },
            GoSourceFile {
                path: "b/b.go",
                content: "package b\nfunc B() {}\n",
            },
            GoSourceFile {
                path: "ignored_mod/go.mod",
                content: "module example.com/ignored\n",
            },
            GoSourceFile {
                path: "ignored_mod/c.go",
                content: "package c\n",
            },
            GoSourceFile {
                path: "vendor/go.mod",
                content: "module example.com/v\n",
            },
            GoSourceFile {
                path: ".hidden/go.work",
                content: "use ./ignored_mod\n",
            },
        ];

        let graph = build_package_graph(&files);
        let active: Vec<&str> = graph
            .modules
            .iter()
            .filter(|m| m.in_workspace)
            .map(|m| m.module_path.as_str())
            .collect();
        assert_eq!(active, vec!["example.com/a", "example.com/b"]);
        assert!(graph.file_to_package.contains_key("a/a.go"));
        assert!(!graph.file_to_package.contains_key("ignored_mod/c.go"));
        assert_eq!(lexically_join_walked("a", "../../outside"), None);
        assert_eq!(lexically_join_walked("a", "../b"), Some("b".to_owned()));
    }

    #[test]
    fn rootless_and_root_package_imports_compute_cleanly() {
        let files = [
            GoSourceFile {
                path: "root.go",
                content: "package rootpkg\nfunc R() {}\n",
            },
            GoSourceFile {
                path: "sub/sub.go",
                content: "package sub\nfunc S() {}\n",
            },
        ];
        let graph = build_package_graph(&files);
        assert_eq!(
            graph.file_to_package.get("root.go").map(String::as_str),
            Some("rootpkg")
        );
        assert_eq!(
            graph.file_to_package.get("sub/sub.go").map(String::as_str),
            Some("sub")
        );
        let mod_files = [
            GoSourceFile {
                path: "go.mod",
                content: "modulefoo example.com/ignored\nmodule example.com/root\ngo1.19\ngo bad\n",
            },
            GoSourceFile {
                path: "embed.go",
                content: "package root\n",
            },
        ];
        let mod_graph = build_package_graph(&mod_files);
        assert_eq!(
            mod_graph
                .file_to_package
                .get("embed.go")
                .map(String::as_str),
            Some("example.com/root")
        );
        assert_eq!(mod_graph.modules[0].go_minor, DEFAULT_GO_MINOR);
    }
}
