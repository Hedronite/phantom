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

    /// Stored count of build-excluded source files, or none when untracked.
    pub fn build_excluded_count(&self) -> Result<Option<u64>, crate::StoreError> {
        Ok(crate::schema::meta_get(&self.conn, "build_excluded")?
            .and_then(|value| value.parse::<u64>().ok()))
    }

    /// Stores the count of build-excluded source files, writing only on change.
    pub fn set_build_excluded_count(&mut self, count: u64) -> Result<bool, crate::StoreError> {
        self.set_meta_on_change("build_excluded", &count.to_string())
    }

    /// Stores encoded `<module_path>\t<ident1> <ident2>...` lines for
    /// build-excluded files so `build_excluded_for_targets` only flags
    /// symbols whose bare name appears in an excluded file of their module.
    pub fn set_build_excluded_terms(&mut self, terms: &str) -> Result<bool, crate::StoreError> {
        self.set_meta_on_change("build_excluded_terms", terms)
    }

    /// Count of build-excluded files relevant to `targets`. Falls back to
    /// [`Self::build_excluded_count`] when per-file terms are absent.
    pub fn build_excluded_for_targets(
        &self,
        targets: &[crate::SymbolFacts],
    ) -> Result<Option<u64>, crate::StoreError> {
        let Some(total) = self.build_excluded_count()? else {
            return Ok(None);
        };
        if total == 0 {
            return Ok(Some(0));
        }
        let Some(terms) = crate::schema::meta_get(&self.conn, "build_excluded_terms")? else {
            return Ok(Some(total));
        };
        if terms.is_empty() {
            return Ok(Some(total));
        }
        Ok(Some(count_matching_excluded_files(&terms, targets)))
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

fn count_matching_excluded_files(terms: &str, targets: &[crate::SymbolFacts]) -> u64 {
    terms
        .lines()
        .filter(|line| line_matches_any_target(line, targets))
        .count() as u64
}

fn line_matches_any_target(line: &str, targets: &[crate::SymbolFacts]) -> bool {
    let Some((mod_path, idents)) = line.split_once('\t') else {
        return false;
    };
    targets.iter().any(|target| {
        target_in_module(target, mod_path) && idents.split(' ').any(|tok| tok == target.name)
    })
}

fn target_in_module(target: &crate::SymbolFacts, mod_path: &str) -> bool {
    if mod_path.is_empty() {
        return true;
    }
    target.package.as_deref().is_some_and(|pkg| {
        pkg == mod_path
            || pkg
                .strip_prefix(mod_path)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
