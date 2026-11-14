//! Read-only lookups for the SCIP scorer (`extract/tests/scip_score.rs`).
//!
//! The scorer's miss buckets read the call refs of one symbol with
//! their receiver hints, and its origin line reads the ids of the
//! `macro_arg` refs. These live beside `impact`'s `symbol_call_refs`,
//! which the scorer no longer uses: the buckets need the hint text,
//! and that helper sits in another lane's file this week.

use std::collections::{BTreeMap, BTreeSet};

use crate::StoreError;

/// One call-shaped ref under a symbol, as the scorer's buckets read
/// it: the ref kind (`call`, `method_call`, `macro`, or `macro_arg`),
/// the text as written, the receiver expression the extractor kept,
/// the receiver-type hint (`type:T`, `ctor:T::f`, `ctor?:T::f`, and
/// later `ret:f` and `ret?:f`), the inline-module scope the ref sits
/// in, and whether an edge names the ref.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreCallRef {
    pub kind: String,
    pub text: String,
    pub receiver: Option<String>,
    pub receiver_type: Option<String>,
    pub scope: String,
    pub placed: bool,
}

/// One row of the `modules` table, for the scorer's shared-module-file
/// bucket and its glob reach: the crate target the module belongs to,
/// its parent row, the module path from the crate root, the file whose
/// text holds it, and the inline-module scope inside that file.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleRow {
    pub target_id: i64,
    pub parent_id: Option<i64>,
    pub path: String,
    pub file_id: Option<i64>,
    pub scope: String,
}

/// One `use` ref row, for the glob and re-export buckets: the file and
/// inline scope it sits in, the path as written (a glob's path stops
/// before `*`), and its alias.
#[derive(Debug, Clone, PartialEq)]
pub struct UseRow {
    pub id: i64,
    pub file_id: i64,
    pub scope: String,
    pub text: String,
    pub alias: Option<String>,
    pub glob: bool,
}

impl crate::Store {
    /// Every call-shaped ref under the symbol `id`, in ref order. A
    /// symbol with no refs answers an empty vector.
    pub fn score_call_refs(&self, id: i64) -> Result<Vec<ScoreCallRef>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT r.kind, r.text, r.receiver, r.receiver_hint, r.scope,
                    EXISTS (SELECT 1 FROM edges e WHERE e.ref_id = r.id)
             FROM refs r
             WHERE r.from_symbol = ?1
               AND r.kind IN ('call', 'method_call', 'macro', 'macro_arg')
             ORDER BY r.id",
        )?;
        let mut rows = statement.query([id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(ScoreCallRef {
                kind: row.get(0)?,
                text: row.get(1)?,
                receiver: row.get(2)?,
                receiver_type: row.get(3)?,
                scope: row.get(4)?,
                placed: row.get::<_, i64>(5)? != 0,
            });
        }
        Ok(out)
    }

    /// Every `modules` row, by id, for the glob reach and the shared
    /// module file read.
    pub fn module_rows(&self) -> Result<BTreeMap<i64, ModuleRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, target_id, parent_id, path, file_id, scope FROM modules ORDER BY id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let file_id: Option<i64> = row.get(4)?;
            out.insert(
                row.get(0)?,
                ModuleRow {
                    target_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    path: row.get(3)?,
                    file_id,
                    scope: row.get(5)?,
                },
            );
        }
        Ok(out)
    }

    /// Every file that carries a module row, with the number of crate
    /// targets claiming it. A count above one is a file the `mod`
    /// declarations reach from more than one crate root (W13 D4).
    pub fn file_target_counts(&self) -> Result<BTreeMap<i64, usize>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT file_id, COUNT(DISTINCT target_id) FROM modules
             WHERE file_id IS NOT NULL GROUP BY file_id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?, row.get::<_, i64>(1)? as usize);
        }
        Ok(out)
    }

    /// Every `use` ref row, for the glob and re-export buckets.
    pub fn use_rows(&self) -> Result<Vec<UseRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, file_id, scope, text, alias, glob FROM refs
             WHERE kind = 'use' ORDER BY id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(UseRow {
                id: row.get(0)?,
                file_id: row.get(1)?,
                scope: row.get(2)?,
                text: row.get(3)?,
                alias: row.get(4)?,
                glob: row.get::<_, i64>(5)? != 0,
            });
        }
        Ok(out)
    }

    /// The target of every `import` edge that carries its `use` ref:
    /// the ref id to the edge's destination kind and row id.
    pub fn import_targets(&self) -> Result<BTreeMap<i64, (String, i64)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT ref_id, dst_kind, dst_id FROM edges
             WHERE kind = 'import' AND ref_id IS NOT NULL",
        )?;
        let mut rows = statement.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?, (row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    /// The module row of every symbol that carries one.
    pub fn symbol_modules(&self) -> Result<BTreeMap<i64, i64>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT id, module_id FROM symbols WHERE module_id IS NOT NULL")?;
        let mut rows = statement.query([])?;
        let mut out = BTreeMap::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?, row.get(1)?);
        }
        Ok(out)
    }

    /// The ids of every ref of one kind. The scorer's origin line asks
    /// for `macro_arg`: an edge whose ref is one of these resolves a
    /// call that sat inside a macro argument.
    pub fn ref_ids_of_kind(&self, kind: &str) -> Result<BTreeSet<i64>, StoreError> {
        let mut statement = self.conn.prepare("SELECT id FROM refs WHERE kind = ?1")?;
        let mut rows = statement.query([kind])?;
        let mut out = BTreeSet::new();
        while let Some(row) = rows.next()? {
            out.insert(row.get(0)?);
        }
        Ok(out)
    }
}
