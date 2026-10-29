//! Rank I/O for the analyze crate: the resolver's inputs, the stored
//! scores, and the map rows (SPEC-w7 G1a).
//!
//! Every query binds its values; none interpolates user text.
//!
//! Having `graph_epoch` bump inside `replace_edges`'s transaction means a
//! reader can trust the pair `(rank_epoch, graph_epoch)` is consistent;
//! a no-change pass replaces nothing, so the epoch does not move there.

use phantom_fractal_lang::Id;

use crate::StoreError;

/// One rank input row for a non-module symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct RankSymbolRow {
    pub id: Id,
    pub file_id: Id,
    /// The symbol's file path, repo-relative.
    pub path: String,
    pub kind: String,
    pub visibility: String,
    pub is_test: bool,
    /// The target kind of the symbol's module: `lib`, `bin`, or the
    /// stored text for any other target kind. `None` for an orphan.
    pub target_kind: Option<String>,
    /// True when the symbol is `fn main` at a bin target's root file.
    pub is_main: bool,
}

/// One `call` or `impl` out-edge of a symbol. The rank weights `call`
/// by its confidence and `impl` by its own rule, so the kind text ships
/// with the row.
#[derive(Debug, Clone, PartialEq)]
pub struct RankEdge {
    pub src_id: Id,
    pub dst_id: Id,
    /// `call` or `impl`, as stored in `edges.kind`.
    pub kind: String,
    pub confidence: f64,
}

/// One file-level `import` edge: the importing file to the imported
/// symbol's file.
#[derive(Debug, Clone, PartialEq)]
pub struct RankImportEdge {
    pub src_file_id: Id,
    pub dst_file_id: Id,
}

/// One map row.
#[derive(Debug, Clone, PartialEq)]
pub struct MapRow {
    pub id: Id,
    pub file_id: Id,
    pub kind: String,
    pub visibility: String,
    pub name: String,
    /// The `owner` of a method; none for other kinds.
    pub owner: Option<String>,
    pub signature: Option<String>,
    pub doc_summary: Option<String>,
    pub start_line: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapFile {
    /// The file row id.
    pub id: Id,
    /// The file path, repo-relative.
    pub path: String,
}

impl crate::Store {
    /// The stored `graph_epoch`.
    pub fn graph_epoch(&self) -> Result<u64, StoreError> {
        Ok(match crate::schema::meta_get(&self.conn, "graph_epoch")? {
            None => 0,
            Some(raw) => raw.parse().unwrap_or(0),
        })
    }

    /// The stored `rank_epoch`.
    pub fn rank_epoch(&self) -> Result<u64, StoreError> {
        Ok(match crate::schema::meta_get(&self.conn, "rank_epoch")? {
            None => 0,
            Some(raw) => raw.parse().unwrap_or(0),
        })
    }

    /// Every non-module symbol with the fields the rank needs.
    pub fn rank_input(&self) -> Result<Vec<RankSymbolRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.file_id, f.path, s.kind, s.visibility, s.is_test,
                    t.kind, s.name, tf.path
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             LEFT JOIN targets t ON t.id = m.target_id
             LEFT JOIN files tf ON tf.path = t.root_file
             WHERE s.kind <> 'module'
             ORDER BY s.id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let target_kind: Option<String> = row.get(6)?;
            let symbol_name: String = row.get(7)?;
            let root_path: Option<String> = row.get(8)?;
            let declared_kind: String = row.get(3)?;
            let file_path: String = row.get(2)?;
            let is_main = declared_kind == "function"
                && symbol_name == "main"
                && root_path.as_deref() == Some(file_path.as_str())
                && target_kind.as_deref() == Some("bin");
            out.push(RankSymbolRow {
                id: row.get(0)?,
                file_id: row.get(1)?,
                path: row.get(2)?,
                kind: row.get(3)?,
                visibility: row.get(4)?,
                is_test: row.get::<_, i64>(5)? != 0,
                target_kind,
                is_main,
            });
        }
        Ok(out)
    }

    /// Every `call` and `impl` out-edge with its kind text and
    /// confidence, ordered by id.
    pub fn rank_call_edges(&self) -> Result<Vec<RankEdge>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT e.src_id, e.dst_id, e.kind, e.confidence
             FROM edges e
             WHERE e.kind IN ('call', 'impl')
             ORDER BY e.id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(RankEdge {
                src_id: row.get(0)?,
                dst_id: row.get(1)?,
                kind: row.get(2)?,
                confidence: row.get(3)?,
            });
        }
        Ok(out)
    }

    /// Every file-level `import` edge as `(importing file, imported
    /// target's file)`: the source id is a modules row id, joined over
    /// `modules.file_id`; the target may be a symbol (its file) or a
    /// module (its file), and both ends reach the files table.
    pub fn rank_import_edges(&self) -> Result<Vec<RankImportEdge>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT sf.id, df.id
             FROM edges e
             JOIN modules m ON m.id = e.src_id
             JOIN files sf ON sf.id = m.file_id
             JOIN modules tm ON tm.id = e.dst_id
             JOIN files df ON df.id = tm.file_id
             WHERE e.kind = 'import'
               AND e.src_kind = 'module'
               AND e.dst_kind = 'module'
             UNION ALL
             SELECT sf.id, df.id
             FROM edges e
             JOIN modules m ON m.id = e.src_id
             JOIN files sf ON sf.id = m.file_id
             JOIN symbols s ON s.id = e.dst_id
             JOIN files df ON df.id = s.file_id
             WHERE e.kind = 'import'
               AND e.src_kind = 'module'
               AND e.dst_kind = 'symbol'
             ORDER BY 1, 2",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(RankImportEdge {
                src_file_id: row.get(0)?,
                dst_file_id: row.get(1)?,
            });
        }
        Ok(out)
    }

    /// Replaces the whole `rank` table with one row per input, in one
    /// transaction, node kinds `symbol` and `file`, and stores the epoch
    /// they belong to.
    pub fn write_rank(
        &mut self,
        epoch: u64,
        symbols: &[(Id, f64)],
        files: &[(Id, f64)],
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM rank", [])?;
        {
            let mut symbol_statement = tx.prepare(
                "INSERT INTO rank(node_kind, node_id, score, epoch)
                 VALUES ('symbol', ?1, ?2, ?3)",
            )?;
            for (id, score) in symbols {
                symbol_statement.execute((id, score, epoch as i64))?;
            }
        }
        {
            let mut file_statement = tx.prepare(
                "INSERT INTO rank(node_kind, node_id, score, epoch)
                 VALUES ('file', ?1, ?2, ?3)",
            )?;
            for (id, score) in files {
                file_statement.execute((id, score, epoch as i64))?;
            }
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('rank_epoch', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [epoch.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every file (id and repo-relative path), plus every non-test,
    /// non-module symbol for the [`crate::Store::map_rows`] callers.
    pub fn files(&self) -> Result<Vec<MapFile>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT id, path FROM files ORDER BY path")?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(MapFile {
                id: row.get(0)?,
                path: row.get(1)?,
            });
        }
        Ok(out)
    }

    /// Every file path (repo-relative), ordered by path.
    pub fn map_file_paths(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.files()?.into_iter().map(|file| file.path).collect())
    }

    /// Every non-test, non-module symbol with the map fields.
    pub fn map_rows(&self) -> Result<Vec<MapRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.file_id, s.kind, s.visibility, s.name, s.owner,
                    s.signature, s.doc_summary, s.start_line
             FROM symbols s
             WHERE s.is_test = 0 AND s.kind <> 'module'
             ORDER BY s.id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(MapRow {
                id: row.get(0)?,
                file_id: row.get(1)?,
                kind: row.get(2)?,
                visibility: row.get(3)?,
                name: row.get(4)?,
                owner: row.get(5)?,
                signature: row.get(6)?,
                doc_summary: row.get(7)?,
                start_line: row.get(8)?,
            });
        }
        Ok(out)
    }
}
