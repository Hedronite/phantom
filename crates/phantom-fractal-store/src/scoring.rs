//! Read-only lookups for the SCIP scorer (`extract/tests/scip_score.rs`).
//!
//! The scorer's miss buckets read the call refs of one symbol with
//! their receiver hints, and its origin line reads the ids of the
//! `macro_arg` refs. These live beside `impact`'s `symbol_call_refs`,
//! which the scorer no longer uses: the buckets need the hint text,
//! and that helper sits in another lane's file this week.

use std::collections::BTreeSet;

use crate::StoreError;

/// One call-shaped ref under a symbol, as the scorer's buckets read
/// it: the ref kind (`call`, `method_call`, `macro`, or `macro_arg`),
/// the text as written, the receiver expression the extractor kept,
/// the receiver-type hint (`type:T`, `ctor:T::f`, `ctor?:T::f`, and
/// later `ret:f` and `ret?:f`), and whether an edge names the ref.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreCallRef {
    pub kind: String,
    pub text: String,
    pub receiver: Option<String>,
    pub receiver_type: Option<String>,
    pub placed: bool,
}

impl crate::Store {
    /// Every call-shaped ref under the symbol `id`, in ref order. A
    /// symbol with no refs answers an empty vector.
    pub fn score_call_refs(&self, id: i64) -> Result<Vec<ScoreCallRef>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT r.kind, r.text, r.receiver, r.receiver_hint,
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
                placed: row.get::<_, i64>(4)? != 0,
            });
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
