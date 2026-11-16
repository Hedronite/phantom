//! Canon §6.3 tables. W3 fills `meta`, `files`, `symbols`, `refs`,
//! `packages`, `targets`, `modules`, and `edges`; the rest stay empty
//! until later packs.

use rusqlite::Connection;

use crate::{PRODUCER, SCHEMA_VERSION, StoreError};

const SCHEMA_SQL: &str = r#"
CREATE TABLE meta(
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE files(
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    lang TEXT NOT NULL,
    blake3 BLOB NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    module_id INTEGER,
    generated INTEGER NOT NULL DEFAULT 0,
    parse_status TEXT NOT NULL
) STRICT;

CREATE TABLE packages(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT,
    manifest_path TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE targets(
    id INTEGER PRIMARY KEY,
    package_id INTEGER NOT NULL REFERENCES packages,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    root_file TEXT NOT NULL
) STRICT;

CREATE TABLE modules(
    id INTEGER PRIMARY KEY,
    target_id INTEGER NOT NULL REFERENCES targets,
    path TEXT NOT NULL,
    file_id INTEGER REFERENCES files,
    parent_id INTEGER REFERENCES modules,
    scope TEXT NOT NULL DEFAULT '',
    UNIQUE(target_id, path)
) STRICT;

CREATE TABLE symbols(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
    module_id INTEGER REFERENCES modules,
    parent_id INTEGER REFERENCES symbols,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qual_name TEXT NOT NULL,
    stable_key TEXT NOT NULL UNIQUE,
    visibility TEXT NOT NULL,
    signature TEXT,
    doc_summary TEXT,
    start_byte INTEGER,
    end_byte INTEGER,
    start_line INTEGER,
    end_line INTEGER,
    is_test INTEGER NOT NULL DEFAULT 0,
    is_entry INTEGER NOT NULL DEFAULT 0,
    scope TEXT NOT NULL DEFAULT '',
    owner TEXT,
    owner_path TEXT,
    trait_name TEXT
) STRICT;
CREATE INDEX symbols_name ON symbols(name);

CREATE VIEW symbols_fts_src AS
SELECT symbols.id AS id,
       symbols.name AS name,
       symbols.qual_name AS qual_name,
       COALESCE(symbols.signature, '') AS signature,
       COALESCE(symbols.doc_summary, '') AS doc_summary,
       files.path AS path
FROM symbols
JOIN files ON files.id = symbols.file_id;

CREATE VIRTUAL TABLE symbols_fts USING fts5(
    name,
    qual_name,
    signature,
    doc_summary,
    path UNINDEXED,
    content='symbols_fts_src',
    content_rowid='id',
    tokenize="unicode61 separators '_:'"
);

CREATE TABLE refs(
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files ON DELETE CASCADE,
    from_symbol INTEGER REFERENCES symbols ON DELETE CASCADE,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    last_segment TEXT NOT NULL,
    line INTEGER,
    col INTEGER,
    receiver_hint TEXT,
    scope TEXT NOT NULL DEFAULT '',
    receiver TEXT,
    shadowed INTEGER NOT NULL DEFAULT 0,
    alias TEXT,
    glob INTEGER NOT NULL DEFAULT 0,
    visibility TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE INDEX refs_last ON refs(last_segment);
CREATE INDEX refs_from ON refs(from_symbol);

CREATE TABLE edges(
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    src_kind TEXT NOT NULL,
    src_id INTEGER NOT NULL,
    dst_kind TEXT NOT NULL,
    dst_id INTEGER NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    reason TEXT NOT NULL,
    ref_id INTEGER REFERENCES refs ON DELETE CASCADE
) STRICT;
CREATE INDEX edges_src ON edges(src_kind, src_id, kind);
CREATE INDEX edges_dst ON edges(dst_kind, dst_id, kind);

CREATE TABLE cochange(
    a INTEGER NOT NULL,
    b INTEGER NOT NULL,
    together REAL NOT NULL,
    n_a REAL NOT NULL,
    n_b REAL NOT NULL,
    lift REAL NOT NULL,
    conf_ab REAL NOT NULL,
    conf_ba REAL NOT NULL,
    last_oid TEXT NOT NULL,
    last_time INTEGER NOT NULL,
    PRIMARY KEY(a, b)
) WITHOUT ROWID, STRICT;

CREATE TABLE file_history(
    path TEXT PRIMARY KEY,
    file_id INTEGER,
    commits REAL NOT NULL,
    last_time INTEGER NOT NULL,
    renamed_from TEXT
) STRICT;

CREATE TABLE git_scan(
    id INTEGER PRIMARY KEY CHECK(id = 1),
    head_oid TEXT,
    oldest_oid TEXT,
    commits_seen INTEGER,
    skipped_bulk INTEGER,
    window_json TEXT
) STRICT;

CREATE TABLE rank(
    node_kind TEXT NOT NULL,
    node_id INTEGER NOT NULL,
    score REAL NOT NULL,
    epoch INTEGER NOT NULL,
    PRIMARY KEY(node_kind, node_id)
) WITHOUT ROWID, STRICT;

CREATE TABLE index_runs(
    id INTEGER PRIMARY KEY,
    started_at INTEGER,
    finished_at INTEGER,
    mode TEXT,
    files_seen INTEGER,
    files_changed INTEGER,
    errors_json TEXT,
    extractor_versions TEXT
) STRICT;
"#;

pub(crate) fn meta_table_exists(conn: &Connection) -> Result<bool, StoreError> {
    #[cfg(test)]
    crate::open_trace::record("meta_table_exists");
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(crate) const TRIGGERS_SQL: &str = r#"
CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
  INSERT INTO symbols_fts(rowid, name, qual_name, signature, doc_summary, path)
  VALUES (
    new.id,
    new.name,
    new.qual_name,
    COALESCE(new.signature, ''),
    COALESCE(new.doc_summary, ''),
    (SELECT path FROM files WHERE id = new.file_id)
  );
END;
CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, qual_name, signature, doc_summary, path)
  VALUES (
    'delete',
    old.id,
    old.name,
    old.qual_name,
    COALESCE(old.signature, ''),
    COALESCE(old.doc_summary, ''),
    (SELECT path FROM files WHERE id = old.file_id)
  );
END;
CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, name, qual_name, signature, doc_summary, path)
  VALUES (
    'delete',
    old.id,
    old.name,
    old.qual_name,
    COALESCE(old.signature, ''),
    COALESCE(old.doc_summary, ''),
    (SELECT path FROM files WHERE id = old.file_id)
  );
  INSERT INTO symbols_fts(rowid, name, qual_name, signature, doc_summary, path)
  VALUES (
    new.id,
    new.name,
    new.qual_name,
    COALESCE(new.signature, ''),
    COALESCE(new.doc_summary, ''),
    (SELECT path FROM files WHERE id = new.file_id)
  );
END;
"#;

pub(crate) fn create(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute_batch(TRIGGERS_SQL)?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('producer', ?1)",
        [PRODUCER],
    )?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

pub(crate) fn check_stamp(conn: &Connection) -> Result<i64, StoreError> {
    #[cfg(test)]
    crate::open_trace::record("check_stamp");
    check_producer(conn)?;
    let found = stamped_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::SchemaNewer { found });
    }
    if found < 1 {
        return Err(StoreError::UnreadableStamp);
    }
    check_user_version(conn, found)?;
    Ok(found)
}

/// The producer row must name this product.
fn check_producer(conn: &Connection) -> Result<(), StoreError> {
    match meta_get(conn, "producer")? {
        None => Err(StoreError::MissingProducer),
        Some(value) if value == PRODUCER => Ok(()),
        Some(_) => Err(StoreError::ForeignProducer),
    }
}

/// The stamped `schema_version`, as a number.
fn stamped_version(conn: &Connection) -> Result<i64, StoreError> {
    let Some(raw) = meta_get(conn, "schema_version")? else {
        return Err(StoreError::UnreadableStamp);
    };
    raw.parse().map_err(|_| StoreError::UnreadableStamp)
}

/// `user_version` must agree with the stamp.
fn check_user_version(conn: &Connection, found: i64) -> Result<(), StoreError> {
    let user_version = user_version(conn)?;
    if user_version != found {
        return Err(StoreError::SchemaMismatch {
            schema_version: found,
            user_version,
        });
    }
    Ok(())
}

/// Moves a version-1 database to version 2 in one transaction. Version 3
/// columns follow on its own step for a v1 database too.
pub(crate) fn migrate_v1_to_v2(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    crate::open_trace::record("migrate_v1_to_v2");
    let tx = conn.transaction()?;
    tx.execute_batch(TRIGGERS_SQL)?;
    tx.execute(
        "INSERT INTO symbols_fts(symbols_fts) VALUES ('rebuild')",
        [],
    )?;
    tx.execute(
        "UPDATE meta SET value = '2' WHERE key = 'schema_version'",
        [],
    )?;
    tx.pragma_update(None, "user_version", 2)?;
    tx.commit()?;
    Ok(())
}

/// The additive columns of version 3, as `(table, column, add)`. A
/// migration on a downgrade-simulated database skips columns that a
/// fresh-create database already has.
const V3_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "symbols",
        "scope",
        "ALTER TABLE symbols ADD COLUMN scope TEXT NOT NULL DEFAULT ''",
    ),
    (
        "symbols",
        "owner",
        "ALTER TABLE symbols ADD COLUMN owner TEXT",
    ),
    (
        "symbols",
        "owner_path",
        "ALTER TABLE symbols ADD COLUMN owner_path TEXT",
    ),
    (
        "symbols",
        "trait_name",
        "ALTER TABLE symbols ADD COLUMN trait_name TEXT",
    ),
    (
        "modules",
        "scope",
        "ALTER TABLE modules ADD COLUMN scope TEXT NOT NULL DEFAULT ''",
    ),
    (
        "refs",
        "scope",
        "ALTER TABLE refs ADD COLUMN scope TEXT NOT NULL DEFAULT ''",
    ),
    (
        "refs",
        "receiver",
        "ALTER TABLE refs ADD COLUMN receiver TEXT",
    ),
    (
        "refs",
        "shadowed",
        "ALTER TABLE refs ADD COLUMN shadowed INTEGER NOT NULL DEFAULT 0",
    ),
    ("refs", "alias", "ALTER TABLE refs ADD COLUMN alias TEXT"),
    (
        "refs",
        "glob",
        "ALTER TABLE refs ADD COLUMN glob INTEGER NOT NULL DEFAULT 0",
    ),
];

/// Adds each `(table, column, add)` the table does not already hold, so a
/// migration on a downgrade-simulated database skips what a fresh create
/// already made.
fn add_columns(
    tx: &rusqlite::Transaction<'_>,
    columns: &[(&str, &str, &str)],
) -> Result<(), StoreError> {
    for (table, column, add) in columns {
        let present: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |row| row.get(0),
        )?;
        if present == 0 {
            tx.execute_batch(add)?;
        }
    }
    Ok(())
}

/// Stamps `meta.schema_version` and `user_version` with `version`.
fn stamp_version(tx: &rusqlite::Transaction<'_>, version: i64) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        [version.to_string()],
    )?;
    tx.pragma_update(None, "user_version", version)?;
    Ok(())
}

/// Moves a version-2 database to version 3 in one transaction. Only
/// nullable or defaulted columns are added; nothing is dropped.
pub(crate) fn migrate_v2_to_v3(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    crate::open_trace::record("migrate_v2_to_v3");
    let tx = conn.transaction()?;
    add_columns(&tx, V3_COLUMNS)?;
    stamp_version(&tx, 3)?;
    tx.commit()?;
    Ok(())
}

/// The additive column of version 4, as `(table, column, add)`: the
/// visibility of a `use` row, so a `pub use` and a private `use` store
/// different rows. Empty for every other ref kind, and for a row a
/// migration carried over (a re-index fills it).
const V4_COLUMNS: &[(&str, &str, &str)] = &[(
    "refs",
    "visibility",
    "ALTER TABLE refs ADD COLUMN visibility TEXT NOT NULL DEFAULT ''",
)];

/// Moves a version-3 database to [`SCHEMA_VERSION`] in one transaction.
/// Only a defaulted column is added; no row changes and nothing is
/// dropped. A migrated `use` row reads as not re-exported until the next
/// index re-extracts it (the extractor version moved with this column).
pub(crate) fn migrate_v3_to_v4(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    crate::open_trace::record("migrate_v3_to_v4");
    let tx = conn.transaction()?;
    add_columns(&tx, V4_COLUMNS)?;
    stamp_version(&tx, SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// One migration step: the highest stamp it applies to, and the move.
type Step = (i64, fn(&mut Connection) -> Result<(), StoreError>);

/// Every step in order. A database stamped `v` takes each step whose
/// number is at least `v`, so a version-2 database skips the first.
const STEPS: &[Step] = &[
    (1, migrate_v1_to_v2),
    (2, migrate_v2_to_v3),
    (3, migrate_v3_to_v4),
];

/// Brings a database stamped `version` up to [`SCHEMA_VERSION`].
pub(crate) fn migrate(conn: &mut Connection, version: i64) -> Result<(), StoreError> {
    for (last, step) in STEPS {
        if version <= *last {
            step(conn)?;
        }
    }
    Ok(())
}

/// The checks and migrations an existing database gets on a write open,
/// after the connection is hardened: it must be ours, its stamp must be
/// readable, and it migrates to the current schema.
pub(crate) fn prepare_existing(conn: &mut Connection) -> Result<(), StoreError> {
    if !meta_table_exists(conn)? {
        return Err(StoreError::ForeignDatabase);
    }
    let version = check_stamp(conn)?;
    migrate(conn, version)
}

pub(crate) fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    match conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get(0)
    }) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Every canon table `row_count` may answer for.
pub(crate) const COUNT_TABLES: &[&str] = &[
    "meta",
    "files",
    "symbols",
    "symbols_fts",
    "packages",
    "targets",
    "modules",
    "refs",
    "edges",
    "cochange",
    "file_history",
    "git_scan",
    "rank",
    "index_runs",
];

pub(crate) fn row_count(conn: &Connection, table: &str) -> Result<i64, StoreError> {
    let allowed = COUNT_TABLES.contains(&table);
    if !allowed {
        return Err(StoreError::UnknownTable);
    }
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |row| row.get(0))?)
}
