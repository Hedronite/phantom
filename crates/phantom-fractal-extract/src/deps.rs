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
    let dirs: HashMap<&str, &str> = packages
        .iter()
        .map(|package| (package.name.as_str(), parent_dir(&package.manifest_path)))
        .collect();
    let workspace: HashMap<String, String> = manifests
        .iter()
        .find(|manifest| manifest.path == "Cargo.toml")
        .map(|root| entries(&root.text, "[workspace.dependencies]"))
        .into_iter()
        .flatten()
        .filter_map(|(key, form)| match form {
            Form::Path(path) => Some((key, inside("", &path)?)),
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
            let named = match form {
                Form::Path(path) => inside(dir, &path),
                Form::Workspace => workspace.get(&key).cloned(),
            };
            let Some(named) = named else {
                continue;
            };
            if key == package.name || dirs.get(key.as_str()) != Some(&named.as_str()) {
                continue;
            }
            let dep = Dep {
                package: package.name.clone(),
                crate_name: key.replace('-', "_"),
                on: key,
                dev,
            };
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
    }
    deps
}

/// How one accepted dependency line names its package.
#[derive(Debug, PartialEq, Eq)]
enum Form {
    /// `path = "…"`, as written.
    Path(String),
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

/// One `B.workspace = true` or single-line `B = { … }` line.
fn entry(line: &str) -> Option<(String, Form)> {
    let (key, value) = line.split_once('=')?;
    let (key, value) = (key.trim(), value.trim());
    if let Some(name) = key.strip_suffix(".workspace") {
        return (is_bare_key(name) && value == "true").then(|| (name.to_owned(), Form::Workspace));
    }
    if !is_bare_key(key) {
        return None;
    }
    let body = value.strip_prefix('{')?.strip_suffix('}')?;
    let mut path = None;
    let mut workspace = false;
    for field in fields(body)? {
        let (name, value) = field.split_once('=')?;
        match (name.trim(), value.trim()) {
            ("path", value) => path = Some(unquote(value)?.to_owned()),
            ("workspace", "true") => workspace = true,
            ("workspace" | "package" | "git", _) => return None,
            _ => {}
        }
    }
    match (path, workspace) {
        (Some(path), false) => Some((key.to_owned(), Form::Path(path))),
        (None, true) => Some((key.to_owned(), Form::Workspace)),
        _ => None,
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
