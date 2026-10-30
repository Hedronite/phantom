//! The root and index-head `meta` keys (W9 D4), written only on change.
//!
//! `root` holds the canonical root of the index; `index_head` holds HEAD
//! at the last index pass on a git root. The `meta` table's value column
//! is `TEXT NOT NULL`, so a `None` head — a root with no git — stores as
//! the empty string, and `index_head` reads that back as `None`.
//!
//! Each setter first reads the stored value and writes only when it
//! differs, so a pass whose roots and head are unchanged writes nothing.

impl crate::Store {
    /// The stored `meta` `root`, or nothing when never written.
    pub fn root(&self) -> Result<Option<String>, crate::StoreError> {
        crate::schema::meta_get(&self.conn, "root")
    }

    /// Stores the canonical root, writing only when the value differs.
    /// Answers `true` when the row was written, `false` when it already
    /// held exactly that value.
    pub fn set_root(&mut self, root: &str) -> Result<bool, crate::StoreError> {
        self.set_meta_on_change("root", root)
    }

    /// HEAD at the last index pass, `None` when never written or when
    /// the last pass found no git head. The empty string stores as the
    /// latter.
    pub fn index_head(&self) -> Result<Option<String>, crate::StoreError> {
        Ok(crate::schema::meta_get(&self.conn, "index_head")?.filter(|text| !text.is_empty()))
    }

    /// Stores HEAD at the last index pass — `None` when the root has no
    /// git — writing only when the value differs. Answers `true` when the
    /// row was written.
    pub fn set_index_head(&mut self, head: Option<&str>) -> Result<bool, crate::StoreError> {
        let stored = head.unwrap_or("");
        self.set_meta_on_change("index_head", stored)
    }

    /// Upserts one `meta` row, first reading the stored value and
    /// answering `false` — writing nothing — when it already matches.
    fn set_meta_on_change(&mut self, key: &str, value: &str) -> Result<bool, crate::StoreError> {
        if crate::schema::meta_get(&self.conn, key)?.as_deref() == Some(value) {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, value),
        )?;
        Ok(true)
    }
}
