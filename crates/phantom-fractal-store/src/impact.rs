//! Read-only facts about stored symbols and refs, for the impact engine
//! (SPEC-w5 G1b). Every query binds its values; none interpolates user
//! text.

use crate::StoreError;

/// What the impact engine reads about one stored symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolFacts {
    pub id: i64,
    pub name: String,
    /// Lowercase stored text: `function`, `struct`, or `method`, among
    /// others.
    pub kind: String,
    pub file: String,
    /// The symbol's start line, or nothing when the row predates it.
    pub line: Option<i64>,
    pub visibility: String,
    /// The trait as written, for a method of a trait impl.
    pub trait_name: Option<String>,
    pub is_test: bool,
    /// The package the symbol's module belongs to, through
    /// `modules.target_id`; `None` for an orphan.
    pub package: Option<String>,
    /// `lib` or `bin` for the symbol's target; `None` for an orphan.
    pub target_kind: Option<String>,
}

impl crate::Store {
    /// The facts of symbol `id`, or nothing when there is no row.
    pub fn symbol_facts(&self, id: i64) -> Result<Option<SymbolFacts>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.name, s.kind, f.path, s.start_line, s.visibility,
                    s.trait_name, s.is_test, p.name, t.kind
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             LEFT JOIN modules m ON m.id = s.module_id
             LEFT JOIN targets t ON t.id = m.target_id
             LEFT JOIN packages p ON p.id = t.package_id
             WHERE s.id = ?1",
        )?;
        let mut rows = statement.query([id])?;
        match rows.next()? {
            None => Ok(None),
            Some(row) => Ok(Some(SymbolFacts {
                id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                file: row.get(3)?,
                line: row.get(4)?,
                visibility: row.get(5)?,
                trait_name: row.get(6)?,
                is_test: row.get::<_, i64>(7)? != 0,
                package: row.get(8)?,
                target_kind: row.get(9)?,
            })),
        }
    }

    /// How many `call` or `method_call` refs end in `name` and carry no
    /// edge.
    pub fn unresolved_named(&self, name: &str) -> Result<i64, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM refs r
             WHERE r.kind IN ('call', 'method_call')
               AND r.last_segment = ?1
               AND NOT EXISTS (SELECT 1 FROM edges e WHERE e.ref_id = r.id)",
            [name],
            |row| row.get(0),
        )?)
    }

    /// How many `macro_arg` refs end in `name`.
    pub fn macro_args_named(&self, name: &str) -> Result<i64, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM refs r
             WHERE r.kind = 'macro_arg' AND r.last_segment = ?1",
            [name],
            |row| row.get(0),
        )?)
    }
}

/// The spans the scorer matches against (SPEC-w10 G1): every stored
/// symbol's identity, file, kind, name, inclusive 1-based line span,
/// and test mark. Read-only; no write path touches it.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolSpan {
    pub id: i64,
    /// The file's row id, what `file_has_module` and the scorer use.
    pub file_id: i64,
    /// The file's path, repo-relative.
    pub path: String,
    /// Lowercase stored kind: `function` for a `fn`, `method` for a
    /// method, and the other kinds the extract writes.
    pub kind: String,
    pub name: String,
    /// Inclusive 1-based span the matcher contains against.
    pub start_line: i64,
    pub end_line: i64,
    /// The test-module or `#[test]` mark, as stored.
    pub is_test: bool,
}

/// One call-shaped ref stored under a symbol, for the scorer's miss
/// buckets. Read-only; no user text or author data.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolCallRef {
    /// The ref kind text: `call`, `method_call`, `macro`, or
    /// `macro_arg`.
    pub kind: String,
    /// The ref text as written.
    pub text: String,
    /// The receiver text the extractor kept, or none.
    pub receiver: Option<String>,
    /// True when an edge named this ref: the call was placed.
    pub placed: bool,
}

impl crate::Store {
    /// Every symbol's span, for the scorer's innermost-span matching.
    pub fn symbol_spans(&self) -> Result<Vec<SymbolSpan>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT s.id, s.file_id, f.path, s.kind, s.name, s.start_line, s.end_line,
                    s.is_test
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             ORDER BY s.id",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolSpan {
                id: row.get(0)?,
                file_id: row.get(1)?,
                path: row.get(2)?,
                kind: row.get(3)?,
                name: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                is_test: row.get::<_, i64>(7)? != 0,
            });
        }
        Ok(out)
    }

    /// True when the `modules` table carries a row for `file_id` —
    /// the file is claimed by some target. A file that answers false
    /// is an orphan at the store level (SPEC-w11 G1a).
    pub fn file_has_module(&self, file_id: i64) -> Result<bool, StoreError> {
        let claimed: i64 = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM modules WHERE file_id = ?1)",
            [file_id],
            |row| row.get(0),
        )?;
        Ok(claimed != 0)
    }

    /// Every call-shaped ref named under `id`, oldest first, with
    /// whether an edge carried it. A symbol with no refs answers an
    /// empty vector.
    pub fn symbol_call_refs(&self, id: i64) -> Result<Vec<SymbolCallRef>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT r.kind, r.text, r.receiver,
                    EXISTS (SELECT 1 FROM edges e WHERE e.ref_id = r.id)
             FROM refs r
             WHERE r.from_symbol = ?1
               AND r.kind IN ('call', 'method_call', 'macro', 'macro_arg')
             ORDER BY r.id",
        )?;
        let mut rows = statement.query([id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(SymbolCallRef {
                kind: row.get(0)?,
                text: row.get(1)?,
                receiver: row.get(2)?,
                placed: row.get::<_, i64>(3)? != 0,
            });
        }
        Ok(out)
    }
}
