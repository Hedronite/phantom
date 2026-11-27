//! Package/crate alias resolution and exact `path:line` lookups (SPEC-w16 G1a).
//!
//! Workspace crate prefixes (`<crate>::foo`) resolve against stored `crate::...`
//! module paths scoped to matching packages or targets, with `-` / `_`
//! normalized. The alias seam (`ScopeFilter` + `AliasRewrite`) is structured so
//! W17 Go import paths can plug into `resolve_alias_targets` without changing
//! store SQL resolution.
//!
//! Exact `path:line` queries (`src/foo.rs:42` or `./src/foo.rs:42`) require
//! [`IndexState::Fresh`]: a stale index refuses them with [`StoreError::StalePathLine`].

use std::collections::BTreeSet;

use phantom_fractal_lang::parse_qual_input;

use crate::StoreError;
use crate::fetch::collect_rows;
use crate::graph::{SymbolMatch, is_file_path, symbol_match_row};

/// Freshness state supplied by a caller that probed the tree on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    /// The index matches the tree on disk.
    Fresh,
    /// The index is stale relative to the tree on disk; line numbers cannot be trusted.
    Stale,
}

/// A parsed `path:line` query with leading `./` normalized away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathLine<'a> {
    /// Repo-relative file path (`/`-separated, leading `./` stripped).
    pub path: &'a str,
    /// 1-based start line number.
    pub line: i64,
}

/// One stored workspace package/target entry exposed for alias diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageAlias {
    /// Package name as stored in `packages.name`.
    pub package: String,
    /// Target name as stored in `targets.name`.
    pub target: String,
    /// Target kind (`lib`, `bin`, or `test`).
    pub kind: String,
}

/// A deterministic rewrite from a package/crate-prefixed query to `crate::...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRewrite {
    /// Matched query prefix before the separator.
    pub prefix: String,
    /// Owning package name.
    pub package: String,
    /// Owning target name (same as `package` for package-wide matches).
    pub target: String,
    /// Rewritten `crate::...` query string.
    pub rewritten: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeFilter {
    Package(i64),
    Target(i64),
    Module(i64),
}

struct StoredTarget {
    target_id: i64,
    package_id: i64,
    package_name: String,
    target_name: String,
    kind: String,
    lang: String,
    module_id: Option<i64>,
    module_path: String,
}

/// Parses an exact 1-based `path:line` query (`src/foo.rs:42` or `./src/foo.rs:42`).
/// Returns `None` for symbol names, `::`-qualified paths, non-file names, or line 0.
pub fn parse_path_line(query: &str) -> Option<PathLine<'_>> {
    let (raw_path, line_str) = query.rsplit_once(':')?;
    let path = valid_file_path(raw_path)?;
    let line = parse_positive_line(line_str)?;
    Some(PathLine { path, line })
}

fn valid_file_path(raw_path: &str) -> Option<&str> {
    if raw_path.ends_with(':') || raw_path.contains("::") {
        return None;
    }
    let path = normalize_rel_path(raw_path);
    (!path.is_empty() && is_file_path(path)).then_some(path)
}

fn parse_positive_line(line_str: &str) -> Option<i64> {
    if !line_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    line_str.parse::<i64>().ok().filter(|&n| n >= 1)
}

/// Strips repeated leading `./` prefixes from a repo-relative path.
pub(crate) fn normalize_rel_path(mut path: &str) -> &str {
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn resolve_alias_targets(
    targets: &[StoredTarget],
    query: &str,
) -> Vec<(ScopeFilter, AliasRewrite)> {
    let rust = rust_crate_aliases(targets, query);
    if !rust.is_empty() {
        return rust;
    }
    go_package_aliases(targets, query)
}

fn rust_crate_aliases(targets: &[StoredTarget], query: &str) -> Vec<(ScopeFilter, AliasRewrite)> {
    let Some((prefix, rest)) = split_crate_prefix(query) else {
        return Vec::new();
    };
    let norm = normalize_crate_name(prefix);
    let rewritten = format!("crate::{rest}");
    let mut out = matching_package_scopes(targets, prefix, &norm, &rewritten);
    collect_target_scopes(targets, prefix, &norm, &rewritten, &mut out);
    out
}

fn go_package_aliases(targets: &[StoredTarget], query: &str) -> Vec<(ScopeFilter, AliasRewrite)> {
    let prefix_matches: Vec<&StoredTarget> = targets
        .iter()
        .filter(|t| is_go_import_prefix(t, query))
        .collect();
    if let Some(best_len) = prefix_matches.iter().map(|t| t.module_path.len()).max() {
        return prefix_matches
            .into_iter()
            .filter(|t| t.module_path.len() == best_len)
            .filter_map(|t| go_rewrite(t, &t.module_path, &query[best_len + 1..]))
            .collect();
    }
    go_short_package_aliases(targets, query)
}

fn is_go_import_prefix(target: &StoredTarget, query: &str) -> bool {
    target.lang == "go"
        && target.module_id.is_some()
        && !target.module_path.is_empty()
        && query
            .strip_prefix(&target.module_path)
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
}

fn go_short_package_aliases(
    targets: &[StoredTarget],
    query: &str,
) -> Vec<(ScopeFilter, AliasRewrite)> {
    let Some((pkg, rest)) = query.split_once('.') else {
        return Vec::new();
    };
    if rest.is_empty() {
        return Vec::new();
    }
    targets
        .iter()
        .filter(|t| t.lang == "go" && t.module_id.is_some() && matches_go_short(t, pkg))
        .filter_map(|t| go_rewrite(t, pkg, rest))
        .collect()
}

fn matches_go_short(target: &StoredTarget, pkg: &str) -> bool {
    !target.module_path.is_empty()
        && (target.target_name == pkg
            || target.module_path.rsplit('/').next() == Some(pkg)
            || (pkg == "main" && target.kind == "bin"))
}

fn go_rewrite(
    target: &StoredTarget,
    prefix: &str,
    rest: &str,
) -> Option<(ScopeFilter, AliasRewrite)> {
    let module_id = target.module_id?;
    let qual = parse_qual_input("go", rest);
    let rewritten = format!("{}::{qual}", target.module_path);
    Some((
        ScopeFilter::Module(module_id),
        AliasRewrite {
            prefix: prefix.to_owned(),
            package: target.package_name.clone(),
            target: target.target_name.clone(),
            rewritten,
        },
    ))
}

fn split_crate_prefix(query: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = query.split_once("::")?;
    (!rest.is_empty() && !matches!(prefix, "crate" | "self" | "super")).then_some((prefix, rest))
}

fn matching_package_scopes(
    targets: &[StoredTarget],
    prefix: &str,
    norm: &str,
    rewritten: &str,
) -> Vec<(ScopeFilter, AliasRewrite)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for t in targets {
        if t.lang != "go"
            && normalize_crate_name(&t.package_name) == norm
            && seen.insert(t.package_id)
        {
            out.push((
                ScopeFilter::Package(t.package_id),
                AliasRewrite {
                    prefix: prefix.to_owned(),
                    package: t.package_name.clone(),
                    target: t.package_name.clone(),
                    rewritten: rewritten.to_owned(),
                },
            ));
        }
    }
    out
}

fn collect_target_scopes(
    targets: &[StoredTarget],
    prefix: &str,
    norm: &str,
    rewritten: &str,
    out: &mut Vec<(ScopeFilter, AliasRewrite)>,
) {
    let mut seen = BTreeSet::new();
    for t in targets {
        if t.lang != "go"
            && normalize_crate_name(&t.package_name) != norm
            && normalize_crate_name(&t.target_name) == norm
            && seen.insert(t.target_id)
        {
            out.push((
                ScopeFilter::Target(t.target_id),
                AliasRewrite {
                    prefix: prefix.to_owned(),
                    package: t.package_name.clone(),
                    target: t.target_name.clone(),
                    rewritten: rewritten.to_owned(),
                },
            ));
        }
    }
}

fn scope_ids(scope: ScopeFilter) -> (Option<i64>, Option<i64>, Option<i64>) {
    match scope {
        ScopeFilter::Package(pkg) => (Some(pkg), None, None),
        ScopeFilter::Target(tgt) => (None, Some(tgt), None),
        ScopeFilter::Module(module_id) => (None, None, Some(module_id)),
    }
}

impl crate::Store {
    /// Stored workspace package and target names for alias resolution and diagnostics.
    pub fn package_aliases(&self) -> Result<Vec<PackageAlias>, StoreError> {
        let mut seen = BTreeSet::new();
        Ok(self
            .stored_targets()?
            .into_iter()
            .filter(|t| seen.insert(t.target_id))
            .map(|t| PackageAlias {
                package: t.package_name,
                target: t.target_name,
                kind: t.kind,
            })
            .collect())
    }

    /// Returns the first deterministic `<crate>::...` -> `crate::...` alias rewrite
    /// matching a stored package or target, or `None` when the prefix is not known.
    pub fn rewrite_crate_alias(&self, query: &str) -> Result<Option<AliasRewrite>, StoreError> {
        let targets = self.stored_targets()?;
        Ok(resolve_alias_targets(&targets, query)
            .into_iter()
            .next()
            .map(|(_, rewrite)| rewrite))
    }

    /// Symbols whose `start_line` matches `line` in `path` (after stripping `./`).
    /// Callers enforcing freshness should use [`Self::symbols_matching_for`].
    pub fn symbols_at_line(&self, path: &str, line: i64) -> Result<Vec<SymbolMatch>, StoreError> {
        let normalized = normalize_rel_path(path);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE f.path = ?1 AND s.start_line = ?2
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query((normalized, line));
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }

    /// Symbols in `path` ordered by distance to `line`, for `not_found` suggestions.
    pub(crate) fn did_you_mean_at_line(
        &self,
        path: &str,
        line: i64,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let normalized = normalize_rel_path(path);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE f.path = ?1
             ORDER BY ABS(COALESCE(s.start_line, 0) - ?2), s.start_line, s.id
             LIMIT 10",
        )?;
        let queried = statement.query((normalized, line));
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }

    pub(crate) fn symbols_matching_alias(
        &self,
        query: &str,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let targets = self.stored_targets()?;
        let scopes = resolve_alias_targets(&targets, query);
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = self.collect_scoped_hits(&scopes)?;
        sort_symbol_matches(&mut out);
        Ok(out)
    }

    fn collect_scoped_hits(
        &self,
        scopes: &[(ScopeFilter, AliasRewrite)],
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for (scope, rewrite) in scopes {
            for hit in self.scoped_hits(*scope, &rewrite.rewritten)? {
                if seen.insert(hit.id) {
                    out.push(hit);
                }
            }
        }
        Ok(out)
    }

    fn scoped_hits(
        &self,
        scope: ScopeFilter,
        rewritten: &str,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let hits = self.scoped_exact(scope, rewritten)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
        self.scoped_method(scope, rewritten)
    }

    fn stored_targets(&self) -> Result<Vec<StoredTarget>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT t.id, p.id, p.name, t.name, t.kind,
                    COALESCE(
                        (SELECT f.lang FROM files f WHERE f.module_id = m.id LIMIT 1),
                        (SELECT f.lang FROM files f WHERE f.id = m.file_id LIMIT 1),
                        (SELECT f.lang FROM files f WHERE f.path = t.root_file LIMIT 1),
                        'rust'
                    ),
                    m.id, COALESCE(m.path, '')
             FROM targets t
             JOIN packages p ON p.id = t.package_id
             LEFT JOIN modules m ON m.target_id = t.id AND m.scope = ''
             ORDER BY t.id, m.id",
        )?;
        let queried = statement.query([]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            Ok(StoredTarget {
                target_id: row.get(0)?,
                package_id: row.get(1)?,
                package_name: row.get(2)?,
                target_name: row.get(3)?,
                kind: row.get(4)?,
                lang: row.get(5)?,
                module_id: row.get(6)?,
                module_path: row.get(7)?,
            })
        })
    }

    fn scoped_exact(
        &self,
        scope: ScopeFilter,
        rewritten: &str,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let qualified = rewritten.strip_prefix("crate::").unwrap_or(rewritten);
        let (pkg_id, tgt_id, mod_id) = scope_ids(scope);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE EXISTS (
                 SELECT 1 FROM modules fm
                 JOIN targets t ON t.id = fm.target_id
                 WHERE (
                     fm.id = COALESCE(s.module_id, f.module_id)
                     OR (f.lang = 'rust' AND fm.file_id = s.file_id AND fm.scope = '')
                 )
                   AND ((?2 IS NOT NULL AND t.package_id = ?2)
                        OR (?3 IS NOT NULL AND t.id = ?3)
                        OR (?4 IS NOT NULL AND fm.id = ?4))
                   AND CASE
                           WHEN f.lang = 'rust' THEN
                               CASE
                                   WHEN (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) = 'crate'
                                       THEN s.qual_name
                                   WHEN (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) LIKE 'crate::%'
                                       THEN substr(CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END, 8) || '::' || s.qual_name
                                   ELSE (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) || '::' || s.qual_name
                               END = ?1
                           WHEN fm.scope = '' THEN fm.path || '::' || s.qual_name = ?5
                           ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) || '::' || s.qual_name = ?5
                       END
             )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query((qualified, pkg_id, tgt_id, mod_id, rewritten));
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }

    fn scoped_method(
        &self,
        scope: ScopeFilter,
        rewritten: &str,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let qualified = rewritten.strip_prefix("crate::").unwrap_or(rewritten);
        let Some((module, name)) = qualified.rsplit_once("::") else {
            return Ok(Vec::new());
        };
        let (pkg_id, tgt_id, mod_id) = scope_ids(scope);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.owner IS NOT NULL AND s.name = ?2
               AND EXISTS (
                   SELECT 1 FROM modules gm
                   JOIN targets t ON t.id = gm.target_id
                   WHERE (
                       gm.id = COALESCE(s.module_id, f.module_id)
                       OR (f.lang = 'rust' AND gm.file_id = s.file_id AND gm.scope = '')
                   )
                     AND ((?3 IS NOT NULL AND t.package_id = ?3)
                          OR (?4 IS NOT NULL AND t.id = ?4)
                          OR (?5 IS NOT NULL AND gm.id = ?5))
                     AND (
                         (CASE WHEN gm.scope = '' THEN gm.path ELSE substr(gm.path, 1, length(gm.path) - length(gm.scope) - 2) END) = ?1
                         OR (
                             length(CASE WHEN gm.scope = '' THEN gm.path ELSE substr(gm.path, 1, length(gm.path) - length(gm.scope) - 2) END) > length(?1) + 2
                             AND substr(CASE WHEN gm.scope = '' THEN gm.path ELSE substr(gm.path, 1, length(gm.path) - length(gm.scope) - 2) END, -length(?1) - 2) = '::' || ?1
                         )
                     )
               )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query((module, name, pkg_id, tgt_id, mod_id));
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }
}

fn sort_symbol_matches(matches: &mut [SymbolMatch]) {
    matches.sort_by(|a, b| {
        a.qual_name
            .cmp(&b.qual_name)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_short_alias_rejects_empty_member_after_dot() {
        let targets = vec![StoredTarget {
            target_id: 1,
            package_id: 1,
            package_name: "gopkg.in/yaml.v3".to_owned(),
            target_name: "yaml".to_owned(),
            kind: "lib".to_owned(),
            lang: "go".to_owned(),
            module_id: Some(1),
            module_path: "gopkg.in/yaml.v3".to_owned(),
        }];
        assert!(resolve_alias_targets(&targets, "yaml.").is_empty());
        assert!(resolve_alias_targets(&targets, "gopkg.in/yaml.v3.").is_empty());
        assert!(resolve_alias_targets(&targets, "gopkg.in/yaml.v3/Decode").is_empty());
        assert!(resolve_alias_targets(&targets, ".Decode").is_empty());
        assert!(resolve_alias_targets(&targets, "a/b.Decode").is_empty());
    }
}
