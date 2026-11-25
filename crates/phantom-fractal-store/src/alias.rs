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
}

struct StoredTarget {
    target_id: i64,
    package_id: i64,
    package_name: String,
    target_name: String,
    kind: String,
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
    rust_crate_aliases(targets, query)
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
        if normalize_crate_name(&t.package_name) == norm && seen.insert(t.package_id) {
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
    for t in targets {
        if normalize_crate_name(&t.package_name) != norm
            && normalize_crate_name(&t.target_name) == norm
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

fn scope_ids(scope: ScopeFilter) -> (Option<i64>, Option<i64>) {
    match scope {
        ScopeFilter::Package(pkg) => (Some(pkg), None),
        ScopeFilter::Target(tgt) => (None, Some(tgt)),
    }
}

impl crate::Store {
    /// Stored workspace package and target names for alias resolution and diagnostics.
    pub fn package_aliases(&self) -> Result<Vec<PackageAlias>, StoreError> {
        Ok(self
            .stored_targets()?
            .into_iter()
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
            "SELECT t.id, p.id, p.name, t.name, t.kind
             FROM targets t
             JOIN packages p ON p.id = t.package_id
             ORDER BY t.id",
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
            })
        })
    }

    fn scoped_exact(
        &self,
        scope: ScopeFilter,
        rewritten: &str,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        let qualified = rewritten.strip_prefix("crate::").unwrap_or(rewritten);
        let (pkg_id, tgt_id) = scope_ids(scope);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE EXISTS (
                 SELECT 1 FROM modules fm
                 JOIN targets t ON t.id = fm.target_id
                 WHERE fm.file_id = s.file_id AND fm.scope = ''
                   AND ((?2 IS NOT NULL AND t.package_id = ?2) OR (?3 IS NOT NULL AND t.id = ?3))
                   AND CASE
                           WHEN fm.path = 'crate' THEN s.qual_name
                           WHEN fm.path LIKE 'crate::%'
                                 THEN substr(fm.path, 8) || '::' || s.qual_name
                           ELSE fm.path || '::' || s.qual_name
                       END = ?1
             )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query((qualified, pkg_id, tgt_id));
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
        let (pkg_id, tgt_id) = scope_ids(scope);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.owner IS NOT NULL AND s.name = ?2
               AND EXISTS (
                   SELECT 1 FROM modules gm
                   JOIN targets t ON t.id = gm.target_id
                   WHERE gm.file_id = s.file_id AND gm.scope = ''
                     AND ((?3 IS NOT NULL AND t.package_id = ?3) OR (?4 IS NOT NULL AND t.id = ?4))
                     AND (
                         gm.path = ?1
                         OR (
                             length(gm.path) > length(?1) + 2
                             AND substr(gm.path, -length(?1) - 2) = '::' || ?1
                         )
                     )
               )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query((module, name, pkg_id, tgt_id));
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
