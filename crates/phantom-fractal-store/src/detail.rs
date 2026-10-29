//! Read-only per-symbol detail reads for `show` (SPEC-w6 G0b). Every
//! query binds its values; none interpolates user text.

use crate::StoreError;

/// One symbol's stored detail row.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolDetail {
    pub id: i64,
    pub stable_key: String,
    pub kind: String,
    pub name: String,
    pub qual_name: String,
    pub visibility: String,
    /// The symbol's file path, repo-relative.
    pub path: String,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
    pub signature: Option<String>,
    pub doc_summary: Option<String>,
    pub is_test: bool,
}

impl crate::Store {
    /// The detail row of symbol `id`, or nothing when there is no row.
    pub fn symbol_detail(&self, id: i64) -> Result<Option<SymbolDetail>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.stable_key, s.kind, s.name, s.qual_name, s.visibility,
                    f.path, s.start_line, s.end_line, s.signature, s.doc_summary, s.is_test
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             WHERE s.id = ?1",
        )?;
        let mut rows = statement.query([id])?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => Ok(Some(SymbolDetail {
                id: row.get(0)?,
                stable_key: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                qual_name: row.get(4)?,
                visibility: row.get(5)?,
                path: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                signature: row.get(9)?,
                doc_summary: row.get(10)?,
                is_test: row.get::<_, i64>(11)? != 0,
            })),
        }
    }

    /// The symbol id behind a stable key, or nothing when unknown.
    pub fn symbol_by_stable_key(&self, key: &str) -> Result<Option<i64>, StoreError> {
        match self.conn.query_row(
            "SELECT id FROM symbols WHERE stable_key = ?1",
            [key],
            |row| row.get(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// For a struct, enum or trait, the stored methods the W4
    /// `self-method` edges attribute to that type: a method row whose
    /// `owner` equals the symbol's `name` (the `impl` self type's last
    /// path segment). Ordered by path, then line. Empty for any other
    /// kind, and for any symbol with no method rows.
    pub fn members(&self, id: i64) -> Result<Vec<SymbolDetail>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT m.id, m.stable_key, m.kind, m.name, m.qual_name, m.visibility,
                    f.path, m.start_line, m.end_line, m.signature, m.doc_summary, m.is_test
             FROM symbols s
             JOIN files fs ON fs.id = s.file_id
             JOIN symbols m ON m.kind = 'method' AND m.owner = s.name
             JOIN files f ON f.id = m.file_id
             WHERE s.id = ?1
               AND s.kind IN ('struct', 'enum', 'trait')
             ORDER BY f.path, m.start_line, m.id",
        )?;
        let mut rows = statement.query([id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolDetail {
                id: row.get(0)?,
                stable_key: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                qual_name: row.get(4)?,
                visibility: row.get(5)?,
                path: row.get(6)?,
                start_line: row.get(7)?,
                end_line: row.get(8)?,
                signature: row.get(9)?,
                doc_summary: row.get(10)?,
                is_test: row.get::<_, i64>(11)? != 0,
            });
        }
        Ok(out)
    }
}
