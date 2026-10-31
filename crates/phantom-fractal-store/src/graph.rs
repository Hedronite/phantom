//! The crate-graph store API for the resolver (SPEC-w3 G1c).
//!
//! Every statement binds its values; none interpolates user text. Ids are
//! the store's row ids and the shapes are the contract types of
//! `phantom_fractal_lang::resolve`.

use std::collections::HashMap;

use phantom_fractal_lang::{
    CallRef, Edge, Id, Link, Module, Reason, ResolveInput, Symbol, SymbolKind, Target, TargetKind,
    UseBinding,
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

/// One stored edge row of one kind, as `edges_of_kind` reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRow {
    pub id: i64,
    pub kind: String,
    pub src_kind: String,
    pub src_id: i64,
    pub dst_kind: String,
    pub dst_id: i64,
    pub confidence: f64,
    pub reason: String,
    /// The `use` ref of an `import` edge; none for an `impl` edge.
    pub ref_id: Option<i64>,
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

/// 64 lowercase hex characters for one fingerprint's bytes.
fn hex_lower(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Decodes 64 lowercase hex characters into 32 bytes, or nothing when
/// the text is any other shape — uppercase, short, or not hex.
fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let data = text.as_bytes();
    if data.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for index in 0..32 {
        let high = hex_value(data[index * 2])?;
        let low = hex_value(data[index * 2 + 1])?;
        out[index] = (high << 4) | low;
    }
    Some(out)
}

pub(crate) fn reason_from_text(reason: &str) -> Result<Reason, StoreError> {
    match reason {
        "path-exact" => Ok(Reason::PathExact),
        "local" => Ok(Reason::Local),
        "use-alias" => Ok(Reason::UseAlias),
        "assoc" => Ok(Reason::Assoc),
        "self-method" => Ok(Reason::SelfMethod),
        "self-assoc" => Ok(Reason::SelfAssoc),
        "crate-path" => Ok(Reason::CratePath),
        "typed-method" => Ok(Reason::TypedMethod),
        "unique-method" => Ok(Reason::UniqueMethod),
        other => Err(StoreError::UnknownReason(other.to_owned())),
    }
}

pub(crate) fn reason_to_text(reason: Reason) -> &'static str {
    match reason {
        Reason::PathExact => "path-exact",
        Reason::Local => "local",
        Reason::UseAlias => "use-alias",
        Reason::Assoc => "assoc",
        Reason::SelfMethod => "self-method",
        Reason::SelfAssoc => "self-assoc",
        Reason::CratePath => "crate-path",
        Reason::TypedMethod => "typed-method",
        Reason::UniqueMethod => "unique-method",
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
    ///
    /// `test_modules` carries the ids of the modules whose symbols are
    /// tests (SPEC-w5 G1b): after the linking step, every symbol whose
    /// module row belongs to that set gets `is_test = 1`. Nothing here
    /// ever clears a symbol's own flag; only what the extract recorded
    /// for the symbol itself may hold zero.
    pub fn write_crate_graph(
        &mut self,
        packages: &[Package],
        targets: &[Target],
        modules: &[Module],
        test_modules: &[Id],
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
        let tested: Vec<i64> = modules
            .iter()
            .filter(|module| test_modules.contains(&module.id))
            .filter_map(|module| module_ids.get(&module.id).copied())
            .collect();
        // Bind one update per test-module row: no id list splicing.
        for stored_id in tested {
            tx.execute(
                "UPDATE symbols SET is_test = 1 WHERE module_id = ?1",
                [stored_id],
            )?;
        }
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
                        s.scope, s.module_id, s.signature, f.path
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
                    signature: row.get(8)?,
                    file: row.get(9)?,
                });
            }
        }
        let mut uses = Vec::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT r.text, r.alias, r.glob, r.scope, f.path,
                        (SELECT m.id FROM modules m WHERE m.file_id = f.id AND m.scope = ''),
                        r.id
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind = 'use' ORDER BY r.id",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                uses.push(UseBinding {
                    id: row.get(6)?,
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
                        r.scope, f.path, r.kind, r.text, r.receiver_hint, r.receiver, r.shadowed
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind IN ('call', 'method_call', 'macro', 'macro_arg') ORDER BY r.id",
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
                    receiver_type: row.get(7)?,
                    receiver: row.get(8)?,
                    shadowed: row.get::<_, i64>(9)? != 0,
                });
            }
        }
        Ok(ResolveInput {
            targets,
            modules,
            symbols,
            uses,
            refs,
            deps: Vec::new(),
        })
    }

    /// Deletes every stored edge, then inserts the given set, all in one
    /// transaction. A graph rebuild: the stored `graph_epoch` increments
    /// here, in the same transaction, and nothing else ever moves it —
    /// a no-change pass calls nothing, so the epoch stands still.
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
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('graph_epoch', 1)
             ON CONFLICT(key) DO UPDATE SET value = value + 1",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Writes the `import` and `impl` links of one index pass into
    /// `edges`: an `import` link is a module row (or the importing
    /// symbol-less module end) to a module or symbol row with
    /// `ref_id` = the `use` ref, an `impl` link is symbol rows with
    /// `ref_id` NULL. The kind text is `LinkKind::as_str` and the reason
    /// text `LinkReason::as_str`; no `call` edge changes, so every
    /// call-edge read keeps its rows and filters.
    pub fn write_links(&mut self, links: &[Link]) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM edges WHERE kind IN ('import', 'impl')", [])?;
        for link in links {
            tx.execute(
                "INSERT INTO edges(
                     kind, src_kind, src_id, dst_kind, dst_id, confidence, reason, ref_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    link.kind.as_str(),
                    link.source.kind(),
                    link.source.id(),
                    link.target.kind(),
                    link.target.id(),
                    link.confidence,
                    link.reason.as_str(),
                    link.ref_id,
                ),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every stored edge of one kind text, as a bounded read ordered by
    /// source then target id.
    pub fn edges_of_kind(&self, kind: &str) -> Result<Vec<EdgeRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT e.id, e.kind, e.src_kind, e.src_id, e.dst_kind, e.dst_id,
                    e.confidence, e.reason, e.ref_id
             FROM edges e
             WHERE e.kind = ?1
             ORDER BY e.src_kind, e.src_id, e.dst_kind, e.dst_id, e.id",
        )?;
        let mut rows = statement.query([kind])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(EdgeRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                src_kind: row.get(2)?,
                src_id: row.get(3)?,
                dst_kind: row.get(4)?,
                dst_id: row.get(5)?,
                confidence: row.get(6)?,
                reason: row.get(7)?,
                ref_id: row.get(8)?,
            });
        }
        Ok(out)
    }

    /// The stored workspace fingerprint, or nothing when it was never
    /// stored or is not 64 lowercase hex characters. A malformed stored
    /// text reads as `None` so a rebuild follows, never an error.
    pub fn workspace_fingerprint(&self) -> Result<Option<[u8; 32]>, StoreError> {
        Ok(
            crate::schema::meta_get(&self.conn, "workspace_fingerprint")?
                .and_then(|value| parse_fingerprint(&value)),
        )
    }

    /// The stored `.rs` skip count, or nothing when it was never stored
    /// or is not decimal text.
    pub fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
        Ok(crate::schema::meta_get(&self.conn, "rs_skipped")?.and_then(|value| value.parse().ok()))
    }

    /// Writes both meta keys in one transaction — the fingerprint as 64
    /// lowercase hex characters, the skip count as decimal text.
    pub fn set_workspace_fingerprint(
        &mut self,
        fingerprint: &[u8; 32],
        rs_skipped: u64,
    ) -> Result<(), StoreError> {
        let hex = hex_lower(fingerprint);
        let skipped = rs_skipped.to_string();
        let tx = self.conn.transaction()?;
        for (key, value) in [
            ("workspace_fingerprint", hex.as_str()),
            ("rs_skipped", skipped.as_str()),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Stores the resolved method-to-type pairs of one index pass: each
    /// pair is `(method_id, owner_id)`, the resolver's attribution of a
    /// method to its `impl` self type. One bound update per pair in one
    /// transaction, and clears every method row first, so a rebuild is
    /// exact: a pass with no resolved pairs leaves the methods
    /// unattributed. `members` reads the result back (SPEC-w6 G0b,
    /// PM ruling: members come from the resolver).
    pub fn write_method_owners(&mut self, pairs: &[(Id, Id)]) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE symbols SET parent_id = NULL WHERE kind = 'method'",
            [],
        )?;
        for (method_id, owner_id) in pairs {
            tx.execute(
                "UPDATE symbols SET parent_id = ?1 WHERE id = ?2",
                (owner_id, method_id),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Resets every symbol stored for `path` to its own test flag, in
    /// stored insert order — items first, then methods, the order a
    /// fresh [`put_source`](crate::Store::put_source) inserts — before
    /// [`write_crate_graph`](crate::Store::write_crate_graph) marks the
    /// test modules (W9 G0 fix, Finding 3). `flags` carries one bool
    /// per stored symbol. A caller reuses each file's own flags from
    /// its fresh extract, so a re-indexed file leaves marks it no
    /// longer earns.
    ///
    /// A `flags` list whose length does not match the file's stored
    /// symbol count fails before anything is written. No existing
    /// signature changes; the caller wires this before the crate
    /// graph's marks.
    pub fn reset_symbol_test_flags(
        &mut self,
        path: &str,
        flags: &[bool],
    ) -> Result<(), StoreError> {
        let file_id: Option<i64> =
            match self
                .conn
                .query_row("SELECT id FROM files WHERE path = ?1", [path], |row| {
                    row.get(0)
                }) {
                Ok(id) => Some(id),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(err) => return Err(err.into()),
            };
        let Some(file_id) = file_id else {
            // No file row stores no symbols: an empty list is a clean
            // no-op, anything else cannot own a reset.
            if flags.is_empty() {
                return Ok(());
            }
            return Err(StoreError::OwnFlagCount {
                stored: 0,
                flags: flags.len(),
            });
        };
        let ids: Vec<i64> = {
            let mut statement = self
                .conn
                .prepare("SELECT id FROM symbols WHERE file_id = ?1 ORDER BY id")?;
            statement
                .query_map([file_id], |row| row.get(0))?
                .collect::<Result<Vec<i64>, _>>()?
        };
        if ids.len() != flags.len() {
            return Err(StoreError::OwnFlagCount {
                stored: ids.len(),
                flags: flags.len(),
            });
        }
        let tx = self.conn.transaction()?;
        for (id, flag) in ids.iter().zip(flags) {
            tx.execute(
                "UPDATE symbols SET is_test = ?1 WHERE id = ?2",
                (i64::from(*flag), id),
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
    /// `ledger::Ledger::open` — the file module path without a leading
    /// `crate::` joined to the in-file `qual_name`, so an inline module
    /// never doubles, and the `crate::` prefixed spelling matches too.
    /// `query` is bound, never spliced.
    pub fn symbols_matching(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        // The optional `crate::` prefix of the query is stripped in the
        // caller, not in SQL, so one bound value covers both spellings.
        let qualified = query.strip_prefix("crate::").unwrap_or(query);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.name = ?1
                OR s.qual_name = ?1
                OR EXISTS (
                    SELECT 1 FROM modules fm
                    WHERE fm.file_id = s.file_id AND fm.scope = ''
                      AND CASE
                              WHEN fm.path = 'crate' THEN s.qual_name
                              WHEN fm.path LIKE 'crate::%'
                                    THEN substr(fm.path, 8) || '::' || s.qual_name
                              ELSE fm.path || '::' || s.qual_name
                          END = ?2
                )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let mut rows = statement.query([query, qualified])?;
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
               AND e.kind = 'call'
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
               AND e.kind = 'call'
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
            let reason = reason_from_text(&row.get::<_, String>(4)?)?;
            out.push(CallHit {
                symbol: row.get(0)?,
                file: row.get(1)?,
                line: row.get(2)?,
                confidence: row.get(3)?,
                reason: reason_to_text(reason).to_owned(),
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
