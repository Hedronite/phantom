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
