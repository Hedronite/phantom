//! The re-export read: which symbols a `pub use` brings into another
//! scope. It reads only rows the store holds — a `use` ref row whose
//! stored visibility is `public`, and the `import` edge that carries
//! that ref to its destination symbol — and writes nothing.
//!
//! A `use` row a version-3 database carried over holds an empty
//! visibility, so it reads as not re-exported until a re-index stores
//! the visibility. A glob re-export (`pub use m::*;`) has no per-symbol
//! `import` edge, so it flags nothing; full re-export resolution is
//! later work.

use std::collections::BTreeSet;

use crate::StoreError;
use crate::fetch::collect_rows;

/// The `use` visibility text a `pub use` stores (the shared spelling of
/// every stored visibility).
const PUBLIC: &str = "public";

impl crate::Store {
    /// The id of every symbol that some `pub use` re-exports: the
    /// destination of each `import` edge whose `use` ref is stored
    /// `public`. A store with no `pub use` answers an empty set.
    pub fn use_reexports(&self) -> Result<BTreeSet<i64>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT e.dst_id
             FROM edges e JOIN refs r ON r.id = e.ref_id
             WHERE e.kind = 'import' AND e.dst_kind = 'symbol'
               AND r.kind = 'use' AND r.visibility = ?1",
        )?;
        let queried = statement.query([PUBLIC]);
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        let mut rows = queried?;
        let ids = collect_rows(&mut rows, |row| Ok(row.get::<_, i64>(0)?))?;
        Ok(ids.into_iter().collect())
    }

    /// Whether some `pub use` re-exports the symbol `id`. An unknown id
    /// answers `false`.
    pub fn is_reexported(&self, id: i64) -> Result<bool, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT EXISTS (
                 SELECT 1
                 FROM edges e JOIN refs r ON r.id = e.ref_id
                 WHERE e.kind = 'import' AND e.dst_kind = 'symbol' AND e.dst_id = ?1
                   AND r.kind = 'use' AND r.visibility = ?2
             )",
        )?;
        let queried = statement.query_row((id, PUBLIC), |row| row.get::<_, i64>(0));
        #[cfg(test)]
        let queried = crate::fault::tick(queried);
        Ok(queried? != 0)
    }
}
