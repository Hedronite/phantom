//! The crate-graph store API for the resolver (SPEC-w3 G1c).
//!
//! Every statement binds its values; none interpolates user text. Ids are
//! the store's row ids and the shapes are the contract types of
//! `phantom_fractal_lang::resolve`.

use std::collections::HashMap;

use phantom_fractal_lang::{
    CallRef, Edge, FileModule, Id, Link, Module, Reason, ResolveInput, Symbol, SymbolKind, Target,
    TargetKind, UseBinding, format_file_display, parse_qual_input,
};

use crate::StoreError;
use crate::fetch::collect_rows;
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

/// One symbol named exactly as `find` asked: the stored name, kind,
/// visibility text, file and start line, items and methods alike.
#[derive(Debug, Clone, PartialEq)]
pub struct FindRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub visibility: String,
    pub file: String,
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

/// One matching row, with the row id read by the caller.
pub(crate) fn symbol_match_row(
    row: &rusqlite::Row<'_>,
    id: i64,
) -> Result<SymbolMatch, StoreError> {
    Ok(SymbolMatch {
        id,
        kind: row.get(1)?,
        module_path: row.get(2)?,
        qual_name: row.get(3)?,
        file: row.get(4)?,
        line: row.get(5)?,
    })
}
/// One named row: name, kind, visibility, file and start line.
fn find_row(row: &rusqlite::Row<'_>, id: i64) -> Result<FindRow, StoreError> {
    Ok(FindRow {
        id,
        name: row.get(1)?,
        kind: row.get(2)?,
        visibility: row.get(3)?,
        file: row.get(4)?,
        line: row.get(5)?,
    })
}
/// True when the first segment of a `display` spelling names a file: it
/// carries a directory separator or a dot, so `src/tui/app.rs` and
/// `build.rs` qualify while `std`, `crate` and `crate::util` do not.
pub(crate) fn is_file_path(segment: &str) -> bool {
    segment.contains('/') || segment.contains('.')
}
pub(crate) fn reason_from_text(reason: &str) -> Result<Reason, StoreError> {
    Reason::from_store_str(reason).ok_or_else(|| StoreError::UnknownReason(reason.to_owned()))
}

pub(crate) fn reason_to_text(reason: Reason) -> &'static str {
    reason.as_str()
}

pub(crate) fn symbol_kind_from_text(kind: &str) -> Result<SymbolKind, StoreError> {
    SymbolKind::from_store_str(kind).ok_or_else(|| StoreError::UnknownKind(kind.to_owned()))
}

pub(crate) fn target_kind_to_text(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Lib => "lib",
        TargetKind::Bin => "bin",
        TargetKind::Test => "test",
    }
}

pub(crate) fn target_kind_from_text(kind: &str) -> Result<TargetKind, StoreError> {
    match kind {
        "lib" => Ok(TargetKind::Lib),
        "bin" => Ok(TargetKind::Bin),
        "test" => Ok(TargetKind::Test),
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
        self.write_crate_graph_with_files(packages, targets, modules, &[], test_modules)
    }

    /// Replaces `packages`, `targets`, and `modules` and applies explicit
    /// `(file, module)` bindings for multi-file packages whose `modules.file_id`
    /// may be `NULL` (SPEC-w17 G2a).
    pub fn write_crate_graph_with_files(
        &mut self,
        packages: &[Package],
        targets: &[Target],
        modules: &[Module],
        file_modules: &[FileModule],
        test_modules: &[Id],
    ) -> Result<(), StoreError> {
        let tx = self.conn.savepoint()?;
        populate_crate_graph(&tx, packages, targets, modules, file_modules, test_modules)?;
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
            let queried = statement.query([]);
            #[cfg(test)]
            let queried = crate::fault::tick(queried);
            let mut rows = queried?;
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
                 FROM modules m LEFT JOIN files f ON f.id = m.file_id
                 ORDER BY m.id",
            )?;
            let queried = statement.query([]);
            #[cfg(test)]
            let queried = crate::fault::tick(queried);
            let mut rows = queried?;
            while let Some(row) = rows.next()? {
                modules.push(Module {
                    id: row.get(0)?,
                    target: row.get(1)?,
                    parent: row.get(2)?,
                    path: row.get(3)?,
                    file: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
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
            let queried = statement.query([]);
            #[cfg(test)]
            let queried = crate::fault::tick(queried);
            let mut rows = queried?;
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
                        COALESCE(
                            f.module_id,
                            (SELECT m.id FROM modules m WHERE m.file_id = f.id AND m.scope = ''
                             ORDER BY m.id LIMIT 1)
                        ),
                        r.id
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind = 'use' ORDER BY r.id",
            )?;
            let queried = statement.query([]);
            #[cfg(test)]
            let queried = crate::fault::tick(queried);
            let mut rows = queried?;
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
                        COALESCE(
                            f.module_id,
                            (SELECT m.id FROM modules m WHERE m.file_id = r.file_id AND m.scope = ''
                             ORDER BY m.id LIMIT 1)
                        ),
                        r.scope, f.path, r.kind, r.text, r.receiver_hint, r.receiver, r.shadowed
                 FROM refs r JOIN files f ON f.id = r.file_id
                 WHERE r.kind IN ('call', 'method_call', 'macro', 'macro_arg', 'embed', 'value_ref')
                 ORDER BY r.id",
            )?;
            let queried = statement.query([]);
            #[cfg(test)]
            let queried = crate::fault::tick(queried);
            let mut rows = queried?;
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
        let tx = self.conn.savepoint()?;
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
        let tx = self.conn.savepoint()?;
        tx.execute(
            "DELETE FROM edges WHERE kind IN ('import', 'impl', 'embeds')",
            [],
        )?;
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
        let queried = statement.query([kind]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
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

    /// The stored `.rs` skip count (compatibility alias for [`Self::source_skipped`]).
    pub fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
        self.source_skipped()
    }

    /// The stored skipped-source count, or nothing when it was never stored
    /// or is not decimal text.
    pub fn source_skipped(&self) -> Result<Option<u64>, StoreError> {
        if let Some(value) = crate::schema::meta_get(&self.conn, "rs_skipped")? {
            return Ok(value.parse().ok());
        }
        Ok(crate::schema::meta_get(&self.conn, "source_skipped")?
            .and_then(|value| value.parse().ok()))
    }

    /// Writes both meta keys in one transaction — the fingerprint as 64
    /// lowercase hex characters, the skip count as decimal text.
    pub fn set_workspace_fingerprint(
        &mut self,
        fingerprint: &[u8; 32],
        source_skipped: u64,
    ) -> Result<(), StoreError> {
        let hex = hex_lower(fingerprint);
        let skipped = source_skipped.to_string();
        let tx = self.conn.savepoint()?;
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
        let tx = self.conn.savepoint()?;
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
        let tx = self.conn.savepoint()?;
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

    /// Symbols matching a query: the bare name, the in-file `qual_name`
    /// such as `Ledger::open`, and the file module path joined to that
    /// name, with or without `crate::`. When those answer nothing, a
    /// method is tried as `<module>::<name>` from any `::` boundary of its
    /// file module path, so `app::watch_dir` resolves while every query
    /// the exact forms answer keeps exactly its rows. When the module
    /// forms answer nothing too, the `display` spelling is tried: the
    /// file, then the in-file `qual_name`, as in
    /// `src/tui/app.rs::App::start_watcher`. `query` is bound, never
    /// spliced.
    pub fn symbols_matching(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        if crate::alias::parse_path_line(query).is_some() {
            return Err(StoreError::StalePathLine);
        }
        let exact = self.symbols_matching_exact(query)?;
        if !exact.is_empty() {
            return Ok(exact);
        }
        self.symbols_matching_fallback(query)
    }

    fn symbols_matching_fallback(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        let method = self.symbols_matching_method(query)?;
        if !method.is_empty() {
            return Ok(method);
        }
        let display = self.symbols_matching_display(query)?;
        if !display.is_empty() {
            return Ok(display);
        }
        self.symbols_matching_alias(query)
    }

    /// Resolves `query` with explicit index freshness. Exact `path:line`
    /// queries are refused with [`StoreError::StalePathLine`] when `state`
    /// is [`IndexState::Stale`].
    pub fn symbols_matching_for(
        &self,
        query: &str,
        state: crate::IndexState,
    ) -> Result<Vec<SymbolMatch>, StoreError> {
        if let Some(pl) = crate::alias::parse_path_line(query) {
            return match state {
                crate::IndexState::Stale => Err(StoreError::StalePathLine),
                crate::IndexState::Fresh => self.symbols_at_line(pl.path, pl.line),
            };
        }
        self.symbols_matching(query)
    }
    /// The spellings the base answers: the bare name, the in-file
    /// qualified name, and the file module path joined to it, `crate::`
    /// optional.
    /// Renders `<file>::<qual>` display for `hit` keyed on its file's language.
    pub fn format_symbol_display(&self, hit: &SymbolMatch) -> Result<String, StoreError> {
        let lang: Option<String> = self
            .conn
            .query_row("SELECT lang FROM files WHERE path = ?1", [&hit.file], |r| {
                r.get(0)
            })
            .ok();
        let lang = lang.as_deref().unwrap_or("rust");
        Ok(format_file_display(lang, &hit.file, &hit.qual_name))
    }

    fn symbols_matching_exact(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        // The optional `crate::` prefix is stripped in Rust, not in SQL,
        // so one bound value covers both spellings.
        let qualified = query.strip_prefix("crate::").unwrap_or(query);
        let go_query = parse_qual_input("go", query);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.name = ?1
                OR s.qual_name = ?1
                OR (f.lang = 'go' AND s.qual_name = ?3)
                OR EXISTS (
                    SELECT 1 FROM modules fm
                    WHERE fm.id = COALESCE(s.module_id, f.module_id)
                      AND CASE
                              WHEN f.lang = 'rust' THEN
                                  CASE
                                      WHEN (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) = 'crate'
                                          THEN s.qual_name
                                      WHEN (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) LIKE 'crate::%'
                                          THEN substr(CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END, 8) || '::' || s.qual_name
                                      ELSE (CASE WHEN fm.scope = '' THEN fm.path ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) END) || '::' || s.qual_name
                                  END
                              WHEN fm.scope = '' THEN fm.path || '::' || s.qual_name
                              ELSE substr(fm.path, 1, length(fm.path) - length(fm.scope) - 2) || '::' || s.qual_name
                          END = CASE WHEN f.lang = 'rust' THEN ?2 ELSE ?3 END
                )
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query([query, qualified, go_query.as_str()]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }
    /// A method addressed as `<module>::<name>`, with the module part
    /// starting at any `::` boundary of the method's file module path.
    /// The caller runs this only when the exact spellings answer nothing,
    /// so a query the base answers keeps exactly its rows.
    fn symbols_matching_method(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        let qualified = query.strip_prefix("crate::").unwrap_or(query);
        let Some((module, name)) = qualified.rsplit_once("::") else {
            return Ok(Vec::new());
        };
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.owner IS NOT NULL AND s.name = ?2
               AND EXISTS (
                   SELECT 1 FROM modules gm
                   WHERE gm.id = COALESCE(s.module_id, f.module_id)
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
        let queried = statement.query([module, name]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }
    /// A symbol addressed by its `display` spelling: a repo-relative file
    /// path, then the in-file `qual_name` (scope, owner and name), and
    /// nothing else: `file.rs::run` names the free fn `run`, never a method
    /// `Type::run`, whose display is `file.rs::Type::run`. So every display
    /// an ambiguous reply shows names exactly one symbol. The caller runs
    /// this only when every other form answers nothing, so a query the
    /// base answers keeps exactly its rows.
    fn symbols_matching_display(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        let Some((file, name)) = query.split_once("::") else {
            return Ok(Vec::new());
        };
        let file = file.strip_prefix("./").unwrap_or(file);
        if name.is_empty() || !is_file_path(file) {
            return Ok(Vec::new());
        }
        let go_name = parse_qual_input("go", name);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE f.path = ?1 AND (s.qual_name = ?2 OR (f.lang = 'go' AND s.qual_name = ?3))
             ORDER BY s.qual_name, f.path, s.id",
        )?;
        let queried = statement.query([file, name, go_name.as_str()]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
    }
    /// Every stored symbol named exactly `name`, items and methods alike,
    /// ordered by file, line, id: the rows `find` answers with. Unlike
    /// `read_items`, this carries methods, which have no item row.
    pub fn symbols_named(&self, name: &str) -> Result<Vec<FindRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.name, s.kind, s.visibility, f.path, s.start_line
             FROM symbols s JOIN files f ON f.id = s.file_id
             WHERE s.name = ?1
             ORDER BY f.path, s.start_line, s.id",
        )?;
        let queried = statement.query([name]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| find_row(row, row.get::<_, i64>(0)?))
    }
    /// Up to ten symbols whose bare name equals the last `::` segment of
    /// `query`, ordered as [`symbols_matching`](Self::symbols_matching)
    /// orders its rows: the candidates a `not_found` reply offers.
    pub fn did_you_mean(&self, query: &str) -> Result<Vec<SymbolMatch>, StoreError> {
        if let Some(pl) = crate::alias::parse_path_line(query) {
            return self.did_you_mean_at_line(pl.path, pl.line);
        }
        let last = query.rsplit([':', '.']).next().unwrap_or(query);
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.kind, COALESCE(m.path, ''), s.qual_name, f.path, s.start_line
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             WHERE s.name = ?1
               AND NOT (f.lang = 'go' AND s.name IN ('init', '_'))
             ORDER BY s.qual_name, f.path, s.id
             LIMIT 10",
        )?;
        let queried = statement.query([last]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        collect_rows(&mut rows, |row| {
            symbol_match_row(row, row.get::<_, i64>(0)?)
        })
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
        let queried = statement.query(id);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
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

fn clear_crate_graph(tx: &rusqlite::Connection) -> Result<(), StoreError> {
    tx.execute("UPDATE symbols SET module_id = NULL", [])?;
    tx.execute("UPDATE files SET module_id = NULL", [])?;
    tx.execute("UPDATE modules SET parent_id = NULL", [])?;
    tx.execute("DELETE FROM modules", [])?;
    tx.execute("DELETE FROM targets", [])?;
    tx.execute("DELETE FROM packages", [])?;
    Ok(())
}

fn insert_packages(
    tx: &rusqlite::Connection,
    packages: &[Package],
) -> Result<HashMap<String, i64>, StoreError> {
    let mut package_ids = HashMap::new();
    for package in packages {
        tx.execute(
            "INSERT INTO packages(name, version, manifest_path) VALUES (?1, ?2, ?3)",
            (&package.name, &package.version, &package.manifest_path),
        )?;
        package_ids.insert(package.name.clone(), tx.last_insert_rowid());
    }
    Ok(package_ids)
}

fn insert_targets(
    tx: &rusqlite::Connection,
    targets: &[Target],
    package_ids: &HashMap<String, i64>,
) -> Result<HashMap<i64, i64>, StoreError> {
    let mut target_ids = HashMap::new();
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
    Ok(target_ids)
}

fn file_ids_map(tx: &rusqlite::Connection) -> Result<HashMap<String, i64>, StoreError> {
    let mut statement = tx.prepare("SELECT path, id FROM files")?;
    let queried = statement.query([]);
    #[cfg(test)]
    let queried = crate::fault::tick(queried);
    let mut rows = queried?;
    let mut map = HashMap::new();
    while let Some(row) = rows.next()? {
        map.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
    }
    Ok(map)
}

fn insert_modules(
    tx: &rusqlite::Connection,
    modules: &[Module],
    target_ids: &HashMap<i64, i64>,
    file_ids: &HashMap<String, i64>,
) -> Result<HashMap<i64, i64>, StoreError> {
    let mut module_ids: HashMap<i64, i64> = HashMap::new();
    for module in modules {
        module_ids.insert(module.id, (module_ids.len() as i64) + 1);
    }
    for module in modules {
        let stored_id = module_ids[&module.id];
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
    Ok(module_ids)
}

fn populate_crate_graph(
    tx: &rusqlite::Connection,
    packages: &[Package],
    targets: &[Target],
    modules: &[Module],
    file_modules: &[FileModule],
    test_modules: &[Id],
) -> Result<(), StoreError> {
    clear_crate_graph(tx)?;
    let package_ids = insert_packages(tx, packages)?;
    let target_ids = insert_targets(tx, targets, &package_ids)?;
    populate_modules(tx, modules, file_modules, &target_ids, test_modules)
}

fn populate_modules(
    tx: &rusqlite::Connection,
    modules: &[Module],
    file_modules: &[FileModule],
    target_ids: &HashMap<i64, i64>,
    test_modules: &[Id],
) -> Result<(), StoreError> {
    let file_ids = file_ids_map(tx)?;
    let module_ids = insert_modules(tx, modules, target_ids, &file_ids)?;
    link_file_modules(tx, file_modules, &file_ids, &module_ids)?;
    mark_test_modules(tx, modules, test_modules, &module_ids)
}

fn link_file_modules(
    tx: &rusqlite::Connection,
    file_modules: &[FileModule],
    file_ids: &HashMap<String, i64>,
    module_ids: &HashMap<i64, i64>,
) -> Result<(), StoreError> {
    link_default_modules(tx)?;
    for mapping in file_modules {
        bind_one_file_module(tx, mapping, file_ids, module_ids)?;
    }
    Ok(())
}

fn link_default_modules(tx: &rusqlite::Connection) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE files SET module_id = (
             SELECT m.id FROM modules m
             WHERE m.file_id = files.id AND m.scope = ''
             ORDER BY m.id LIMIT 1
         )",
        [],
    )?;
    tx.execute(
        "UPDATE symbols SET module_id = (
             SELECT m.id FROM modules m
             WHERE m.file_id = symbols.file_id AND m.scope = symbols.scope
             ORDER BY m.id LIMIT 1
         )",
        [],
    )?;
    Ok(())
}

fn bind_one_file_module(
    tx: &rusqlite::Connection,
    mapping: &FileModule,
    file_ids: &HashMap<String, i64>,
    module_ids: &HashMap<i64, i64>,
) -> Result<(), StoreError> {
    let stored_module_id = *module_ids
        .get(&mapping.module)
        .ok_or(StoreError::UnknownTarget(mapping.module))?;
    let Some(&file_id) = file_ids.get(&mapping.file) else {
        return Ok(());
    };
    tx.execute(
        "UPDATE files SET module_id = ?1 WHERE id = ?2",
        (stored_module_id, file_id),
    )?;
    tx.execute(
        "UPDATE symbols SET module_id = ?1
         WHERE file_id = ?2
           AND scope = COALESCE((SELECT scope FROM modules WHERE id = ?1), '')",
        (stored_module_id, file_id),
    )?;
    Ok(())
}

fn mark_test_modules(
    tx: &rusqlite::Connection,
    modules: &[Module],
    test_modules: &[Id],
    module_ids: &HashMap<i64, i64>,
) -> Result<(), StoreError> {
    let tested: Vec<i64> = modules
        .iter()
        .filter(|module| test_modules.contains(&module.id))
        .filter_map(|module| module_ids.get(&module.id).copied())
        .collect();
    for stored_id in tested {
        tx.execute(
            "UPDATE symbols SET is_test = 1 WHERE module_id = ?1",
            [stored_id],
        )?;
    }
    Ok(())
}
