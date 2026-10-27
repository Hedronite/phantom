//! The crate-graph store API for the resolver (SPEC-w3 G1c).
//!
//! Every statement binds its values; none interpolates user text. Ids are
//! the store's row ids and the shapes are the contract types of
//! `phantom_fractal_lang::resolve`.

use std::collections::HashMap;

use phantom_fractal_lang::{
    CallRef, Edge, Module, Reason, ResolveInput, Symbol, SymbolKind, Target, TargetKind, UseBinding,
};

use crate::StoreError;
use crate::rows;

/// What `write_crate_graph` replaces the `packages` table with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    pub manifest_path: String,
}

/// One symbol found by [`Store::symbols_matching`](crate::Store::symbols_matching).
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolMatch {
    pub id: i64,
    /// Lowercase stored text: `function`, `struct`, or `method`, among
    /// others.
    pub kind: String,
    /// The module path from the crate root, or nothing for an orphan.
    pub module_path: String,
    pub qual_name: String,
    pub file: String,
    /// The symbol's start line, or nothing when the row predates it.
    pub line: Option<i64>,
}

/// One edge as [`callers`](crate::Store::callers) or
/// [`callees`](crate::Store::callees) returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct CallHit {
    /// The other symbol's row id: the callee for `callers`, the caller
    /// for `callees`.
    pub symbol: i64,
    pub file: String,
    /// The call-site line, when the edge carries its ref.
    pub line: Option<i64>,
    pub confidence: f64,
    pub reason: String,
}

pub(crate) fn reason_to_text(reason: Reason) -> &'static str {
    match reason {
        Reason::PathExact => "path-exact",
        Reason::Local => "local",
        Reason::UseAlias => "use-alias",
        Reason::Assoc => "assoc",
    }
}

pub(crate) fn symbol_kind_from_text(kind: &str) -> Result<SymbolKind, StoreError> {
    match kind {
        "function" => Ok(SymbolKind::Function),
        "struct" => Ok(SymbolKind::Struct),
        "enum" => Ok(SymbolKind::Enum),
        "trait" => Ok(SymbolKind::Trait),
        "module" => Ok(SymbolKind::Module),
        "method" => Ok(SymbolKind::Method),
        other => Err(StoreError::UnknownKind(other.to_owned())),
    }
}

pub(crate) fn target_kind_to_text(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Lib => "lib",
        TargetKind::Bin => "bin",
    }
}

pub(crate) fn target_kind_from_text(kind: &str) -> Result<TargetKind, StoreError> {
    match kind {
        "lib" => Ok(TargetKind::Lib),
        "bin" => Ok(TargetKind::Bin),
        other => Err(StoreError::UnknownKind(other.to_owned())),
    }
}

impl crate::Store {
    /// Replaces the `packages`, `targets`, and `modules` tables with the
    /// given rows in one transaction, then points `files.module_id` and
    /// `symbols.module_id` at the new module rows — by file for files and
    /// by file plus inline scope for symbols. An orphan keeps `NULL`.
    pub fn write_crate_graph(
        &mut self,
        packages: &[Package],
        targets: &[Target],
        modules: &[Module],
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        // Module ids are referenced from files and symbols with foreign
        // keys; a replace first clears both, then deletes the tables.
        tx.execute("UPDATE symbols SET module_id = NULL", [])?;
        tx.execute("UPDATE files SET module_id = NULL", [])?;
        tx.execute("UPDATE modules SET parent_id = NULL", [])?;
        tx.execute("DELETE FROM modules", [])?;
        tx.execute("DELETE FROM targets", [])?;
        tx.execute("DELETE FROM packages", [])?;
        let mut package_ids: HashMap<String, i64> = HashMap::new();
        for package in packages {
            tx.execute(
                "INSERT INTO packages(name, version, manifest_path) VALUES (?1, ?2, ?3)",
                (&package.name, &package.version, &package.manifest_path),
            )?;
            package_ids.insert(package.name.clone(), tx.last_insert_rowid());
        }
        let mut target_ids: HashMap<i64, i64> = HashMap::new();
        for target in targets {
            let package_id = *package_ids
                .get(&target.package)
                .ok_or_else(|| StoreError::UnknownPackage(target.package.clone()))?;
            tx.execute(
                "INSERT INTO targets(package_id, kind, name, root_file) VALUES (?1, ?2, ?3, ?4)",
                (
                    package_id,
                    target_kind_to_text(target.kind),
                    &target.name,
                    &target.root,
                ),
            )?;
            target_ids.insert(target.id, tx.last_insert_rowid());
        }
        let file_ids: HashMap<String, i64> = {
            let mut statement = tx.prepare("SELECT path, id FROM files")?;
            let mut rows = statement.query([])?;
            let mut map = HashMap::new();
            while let Some(row) = rows.next()? {
                map.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
            }
            map
        };
        // First pass over the module ids so a parent never needs an id
        // that is not yet written; input ids must be unique.
        let mut module_ids: HashMap<i64, i64> = HashMap::new();
        for module in modules {
            module_ids.insert(module.id, (module_ids.len() as i64) + 1);
        }
        for module in modules {
            let stored_id = *module_ids
                .get(&module.id)
                .ok_or_else(|| StoreError::UnknownTarget(module.target))?;
            let target_id = *target_ids
                .get(&module.target)
                .ok_or_else(|| StoreError::UnknownTarget(module.target))?;
            let parent_id = module
                .parent
                .and_then(|parent| module_ids.get(&parent).copied());
            tx.execute(
                "INSERT INTO modules(id, target_id, parent_id, path, file_id, scope)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    stored_id,
                    target_id,
                    parent_id,
                    &module.path,
                    file_ids.get(&module.file).copied(),
                    &module.scope,
                ),
            )?;
        }
        tx.execute(
            "UPDATE files SET module_id = (
                 SELECT m.id FROM modules m
                 WHERE m.file_id = files.id AND m.scope = ''
             )",
            [],
        )?;
        tx.execute(
            "UPDATE symbols SET module_id = (
                 SELECT m.id FROM modules m
                 WHERE m.file_id = symbols.file_id AND m.scope = symbols.scope
             )",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Builds the resolver's input from stored rows, in row-id order.
    pub fn resolve_input(&self) -> Result<ResolveInput, StoreError> {
        let mut targets = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT t.id, p.name, t.name, t.kind, t.root_file
                 FROM targets t JOIN packages p ON p.id = t.package_id
                 ORDER BY t.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                targets.push(Target {
                    id: row.get(0)?,
                    package: row.get(1)?,
                    name: row.get(2)?,
                    kind: target_kind_from_text(&row.get::<_, String>(3)?)?,
                    root: row.get(4)?,
                });
            }
        }
        let mut modules = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT m.id, m.target_id, m.parent_id, m.path, f.path, m.scope
                 FROM modules m JOIN files f ON f.id = m.file_id
                 ORDER BY m.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                modules.push(Module {
                    id: row.get(0)?,
                    target: row.get(1)?,
                    parent: row.get(2)?,
                    path: row.get(3)?,
                    file: row.get(4)?,
                    scope: row.get(5)?,
                });
            }
        }
        let mut symbols = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT s.id, s.kind, s.name, s.owner, s.owner_path, s.trait_name,
                        s.scope, s.module_id, f.path
                 FROM symbols s JOIN files f ON f.id = s.file_id
                 ORDER BY s.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                symbols.push(Symbol {
                    id: row.get(0)?,
                    kind: symbol_kind_from_text(&row.get::<_, String>(1)?)?,
                    name: row.get(2)?,
                    owner: row.get(3)?,
                    owner_path: row.get(4)?,
                    trait_name: row.get(5)?,
                    scope: row.get(6)?,
                    module: row.get(7)?,
                    file: row.get(8)?,
                });
            }
        }
        let mut uses = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT r.text, r.alias, r.glob, r.scope, f.path,
                        (SELECT m.id FROM modules m WHERE m.file_id = f.id AND m.scope = '')
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind = 'use' ORDER BY r.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                uses.push(UseBinding {
                    module: row.get(5)?,
                    scope: row.get(3)?,
                    file: row.get(4)?,
                    path: row.get(0)?,
                    alias: row.get(1)?,
                    glob: row.get::<_, i64>(2)? != 0,
                });
            }
        }
        let mut refs = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT r.id, r.from_symbol,
                        (SELECT m.id FROM modules m WHERE m.file_id = r.file_id AND m.scope = ''),
                        r.scope, f.path, r.kind, r.text, r.shadowed
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind IN ('call', 'method_call', 'macro') ORDER BY r.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                refs.push(CallRef {
                    id: row.get(0)?,
                    from: row.get(1)?,
                    module: row.get(2)?,
                    scope: row.get(3)?,
                    file: row.get(4)?,
                    kind: rows::ref_kind_from_text(&row.get::<_, String>(5)?)?,
                    text: row.get(6)?,
                    shadowed: row.get::<_, i64>(7)? != 0,
                });
            }
        }
        Ok(ResolveInput {
            targets,
            modules,
            symbols,
            uses,
            refs,
        })
    }

    /// Deletes every stored edge, then inserts the given set, all in one
    /// transaction.
    pub fn replace_edges(&mut self, edges: &[Edge]) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM edges", [])?;
        for edge in edges {
            tx.execute(
                "INSERT INTO edges(
                     kind, src_kind, src_id, dst_kind, dst_id, confidence, reason, ref_id
                 ) VALUES ('call', 'symbol', ?1, 'symbol', ?2, ?3, ?4, ?5)",
                (
                    edge.source,
                    edge.target,
                    edge.confidence,
                    reason_to_text(edge.reason),
                    edge.ref_id,
                ),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The stored extractor version, or nothing when it was never set.
    pub fn extractor_version(&self) -> Result<Option<u32>, StoreError> {
        match crate::schema::meta_get(&self.conn, "extractor_version")? {
            None => Ok(None),
            Some(raw) => raw
                .parse()
                .map(Some)
                .map_err(|_| StoreError::UnreadableStamp),
        }
    }

    /// Stores the extractor version.
    pub fn set_extractor_version(&mut self, version: u32) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES ('extractor_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [version.to_string()],
        )?;
        Ok(())
    }

    /// Symbols matching a bare name such as `open_ledger` or a
    /// module-qualified name such as `ledger::open_ledger` or
    /// `ledger::Ledger::open`. `query` is bound, never spliced.
    pub fn symbols_matching(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.name = ?1
                OR s.qual_name = ?1
                OR (m.path IS NOT NULL AND m.path || '::' || s.qual_name = ?1)
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let mut rows = statement.query([query])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolMatch {
                id: row.get(0)?,
                kind: row.get(1)?,
                module_path: row.get(2)?,
                qual_name: row.get(3)?,
                file: row.get(4)?,
                line: row.get(5)?,
            });
        }
        Ok(out)
    }

    /// Every edge addressed to the symbol `id`, as the caller's row id,
    /// the caller's file, the call-site line, the confidence, and the
    /// reason.
    pub fn callers(&self, id: i64) -> Result<Vec<CallHit>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT e.src_id, f.path, r.line, e.confidence, e.reason
             FROM edges e
             JOIN refs r ON r.id = e.ref_id
             JOIN files f ON f.id = r.file_id
             WHERE e.dst_id = ?1 AND e.src_kind = 'symbol' AND e.dst_kind = 'symbol'
             ORDER BY f.path, r.line",
        )?;
        Self::collect_hits(&mut statement, [id])
    }

    /// Every edge from the symbol `id`, as the callee's row id, the
    /// callee's file, the call-site line, the confidence, and the reason.
    pub fn callees(&self, id: i64) -> Result<Vec<CallHit>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT e.dst_id, f.path, r.line, e.confidence, e.reason
             FROM edges e
             JOIN refs r ON r.id = e.ref_id
             JOIN symbols s ON s.id = e.dst_id
             JOIN files f ON f.id = s.file_id
             WHERE e.src_id = ?1 AND e.src_kind = 'symbol' AND e.dst_kind = 'symbol'
             ORDER BY f.path, r.line",
        )?;
        Self::collect_hits(&mut statement, [id])
    }

    fn collect_hits(
        statement: &mut rusqlite::Statement<'_>,
        id: [i64; 1],
    ) -> Result<Vec<CallHit>, StoreError> {
        let mut rows = statement.query(id)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(CallHit {
                symbol: row.get(0)?,
                file: row.get(1)?,
                line: row.get(2)?,
                confidence: row.get(3)?,
                reason: row.get(4)?,
            });
        }
        Ok(out)
    }

    /// How many call refs sit inside the symbol `id` without an edge.
    pub fn unresolved_calls(&self, id: i64) -> Result<i64, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM refs r
             WHERE r.from_symbol = ?1 AND r.kind IN ('call', 'method_call', 'macro')
               AND NOT EXISTS (SELECT 1 FROM edges e WHERE e.ref_id = r.id)",
            [id],
            |row| row.get(0),
        )?)
    }
}
