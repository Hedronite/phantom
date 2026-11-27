//! Which workspace libs each package may call by crate name (SPEC-w4 G2a).
//!
//! Each manifest is scanned line by line: no TOML crate. Package `A` may
//! call workspace package `B` only for these forms in `A`'s
//! `[dependencies]`, or in its `[dev-dependencies]`, whose entries are
//! marked dev and serve `A`'s test targets only (SPEC-w11 G1b):
//!
//! - one line `B = { … path = "…" … }` whose path, from `A`'s directory,
//!   stays inside the root and is `B`'s package directory;
//! - `B.workspace = true`, or one line `B = { workspace = true … }`, when the
//!   root `Cargo.toml`'s `[workspace.dependencies]` has one line
//!   `B = { … path = "…" … }` that is `B`'s package directory.
//!
//! Anything else gives no dependency: a `[dependencies.B]` table, a
//! `package = "…"` rename, a `git` source, an inline table over several
//! lines, a version-only line, a quoted key, and every other section,
//! `[build-dependencies]`, `[target.….dependencies]` and
//! `[target.….dev-dependencies]` included. A bin or test calling its own
//! package's lib is not a dependency; the resolver allows it from the
//! target kind.

use std::collections::HashMap;

use phantom_fractal_lang::Dep;

use crate::graph::{self, Manifest, join, normalize, parent_dir};

/// The `deps` the resolver reads, from the walked manifests in walk order.
/// Packages are named as [`graph::build`] names them.
pub fn scan(manifests: &[Manifest]) -> Vec<Dep> {
    let packages = graph::build(manifests, &[]).packages;
    let libs = graph::lib_names(manifests);
    let dirs: HashMap<&str, &str> = packages
        .iter()
        .map(|package| (package.name.as_str(), parent_dir(&package.manifest_path)))
        .collect();
    let workspace: HashMap<String, TargetSpec> = manifests
        .iter()
        .find(|manifest| manifest.path == "Cargo.toml")
        .map(|root| entries(&root.text, "[workspace.dependencies]"))
        .into_iter()
        .flatten()
        .filter_map(|(key, form)| match form {
            Form::Path { path, package } => {
                let target = inside("", &path)?;
                let renamed = package.is_some();
                let real = package.unwrap_or_else(|| key.clone());
                Some((
                    key,
                    TargetSpec {
                        target,
                        real,
                        renamed,
                    },
                ))
            }
            Form::Workspace => None,
        })
        .collect();
    let mut deps: Vec<Dep> = Vec::new();
    for package in &packages {
        let Some(manifest) = manifests
            .iter()
            .find(|manifest| manifest.path == package.manifest_path)
        else {
            continue;
        };
        let dir = parent_dir(&package.manifest_path);
        let sections = [("[dependencies]", false), ("[dev-dependencies]", true)];
        for (dev, (key, form)) in sections.into_iter().flat_map(|(header, dev)| {
            entries(&manifest.text, header)
                .into_iter()
                .map(move |entry| (dev, entry))
        }) {
            let Some(spec) = resolve_target(&key, dir, form, &workspace) else {
                continue;
            };
            if spec.real == package.name
                || dirs.get(spec.real.as_str()) != Some(&spec.target.as_str())
            {
                continue;
            }
            let crate_name = crate_name_for(&key, &spec, &libs);
            let dep = Dep {
                package: package.name.clone(),
                crate_name,
                on: spec.real,
                dev,
            };
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }
    deps
}

#[derive(Clone)]
struct TargetSpec {
    target: String,
    real: String,
    renamed: bool,
}

fn crate_name_for(key: &str, spec: &TargetSpec, libs: &HashMap<String, String>) -> String {
    if spec.renamed {
        return key.replace('-', "_");
    }
    libs.get(spec.real.as_str())
        .cloned()
        .unwrap_or_else(|| key.replace('-', "_"))
}

fn resolve_target(
    key: &str,
    dir: &str,
    form: Form,
    workspace: &HashMap<String, TargetSpec>,
) -> Option<TargetSpec> {
    match form {
        Form::Path { path, package } => Some(TargetSpec {
            target: inside(dir, &path)?,
            renamed: package.is_some(),
            real: package.unwrap_or_else(|| key.to_owned()),
        }),
        Form::Workspace => workspace.get(key).cloned(),
    }
}

/// How one accepted dependency line names its package.
#[derive(Debug, PartialEq, Eq)]
enum Form {
    /// `path = "…"`, with optional `package = "…"` rename.
    Path {
        path: String,
        package: Option<String>,
    },
    /// `workspace = true`.
    Workspace,
}

/// The accepted dependency lines directly under the `header` table.
fn entries(text: &str, header: &str) -> Vec<(String, Form)> {
    let mut inside_table = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') {
            inside_table = line == header;
            continue;
        }
        if inside_table && let Some(entry) = entry(line) {
            out.push(entry);
        }
    }
    out
}

#[derive(Default)]
struct InlineFields {
    path: Option<String>,
    package: Option<String>,
    workspace: bool,
}

/// One `B.workspace = true` or single-line `B = { … }` line.
fn entry(line: &str) -> Option<(String, Form)> {
    let (key, value) = line.split_once('=')?;
    let (key, value) = (key.trim(), value.trim());
    if let Some(name) = key.strip_suffix(".workspace") {
        return (is_bare_key(name) && value == "true").then(|| (name.to_owned(), Form::Workspace));
    }
    inline_entry(key, value)
}

fn inline_entry(key: &str, value: &str) -> Option<(String, Form)> {
    if !is_bare_key(key) {
        return None;
    }
    let body = value.strip_prefix('{')?.strip_suffix('}')?;
    let parsed = parse_inline_fields(body)?;
    form_from_fields(key, parsed)
}

fn form_from_fields(key: &str, parsed: InlineFields) -> Option<(String, Form)> {
    match (parsed.path, parsed.package, parsed.workspace) {
        (Some(path), package, false) => Some((key.to_owned(), Form::Path { path, package })),
        (None, None, true) => Some((key.to_owned(), Form::Workspace)),
        _ => None,
    }
}

fn parse_inline_fields(body: &str) -> Option<InlineFields> {
    let mut parsed = InlineFields::default();
    for field in fields(body)? {
        let (name, value) = field.split_once('=')?;
        if !apply_inline_field(&mut parsed, name.trim(), value.trim()) {
            return None;
        }
    }
    Some(parsed)
}

fn apply_inline_field(parsed: &mut InlineFields, name: &str, value: &str) -> bool {
    match name {
        "path" => {
            parsed.path = unquote(value).map(str::to_owned);
            parsed.path.is_some()
        }
        "package" => {
            parsed.package = non_empty_unquote(value);
            parsed.package.is_some()
        }
        "workspace" => {
            parsed.workspace = value == "true";
            parsed.workspace
        }
        "git" => false,
        _ => true,
    }
}

/// An inline table's body split at its top-level commas, or none when a
/// string or bracket is left open.
fn fields(body: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut quote = None;
    let mut depth = 0_usize;
    let mut start = 0;
    for (at, ch) in body.char_indices() {
        match (quote, ch) {
            (Some(open), ch) if ch == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth = depth.checked_sub(1)?,
            (None, ',') if depth == 0 => {
                out.push(&body[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    if quote.is_some() || depth != 0 {
        return None;
    }
    out.push(&body[start..]);
    Some(
        out.into_iter()
            .filter(|field| !field.trim().is_empty())
            .collect(),
    )
}

fn non_empty_unquote(value: &str) -> Option<String> {
    unquote(value).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The text of a `"…"` or `'…'` string with no quote inside.
fn unquote(value: &str) -> Option<&str> {
    let quote = value
        .chars()
        .next()
        .filter(|ch| *ch == '"' || *ch == '\'')?;
    let inner = value.strip_prefix(quote)?.strip_suffix(quote)?;
    (!inner.contains(quote)).then_some(inner)
}

fn is_bare_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// The directory `path` names from `dir`, if it is relative and stays
/// inside the root.
fn inside(dir: &str, path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    normalize(&join(dir, path))
}
