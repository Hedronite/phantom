//! Canon §6.3 tables. A later pack fills every table except `meta`, `files`,
//! and `symbols`.

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
    is_entry INTEGER NOT NULL DEFAULT 0
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
    receiver_hint TEXT
) STRICT;
CREATE INDEX refs_last ON refs(last_segment);

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

pub(crate) const EMPTY_TABLES: &[&str] = &[
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
    match meta_get(conn, "producer")? {
        None => return Err(StoreError::MissingProducer),
        Some(value) if value == PRODUCER => {}
        Some(_) => return Err(StoreError::ForeignProducer),
    }
    let Some(raw) = meta_get(conn, "schema_version")? else {
        return Err(StoreError::UnreadableStamp);
    };
    let found: i64 = raw.parse().map_err(|_| StoreError::UnreadableStamp)?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::SchemaNewer { found });
    }
    if found != 1 && found != SCHEMA_VERSION {
        return Err(StoreError::UnreadableStamp);
    }
    let user_version = user_version(conn)?;
    if user_version != found {
        return Err(StoreError::SchemaMismatch {
            schema_version: found,
            user_version,
        });
    }
    Ok(found)
}

/// Moves a version-1 database to [`SCHEMA_VERSION`] in one transaction.
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
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        [SCHEMA_VERSION.to_string()],
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
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

pub(crate) fn row_count(conn: &Connection, table: &str) -> Result<i64, StoreError> {
    let allowed = EMPTY_TABLES
        .iter()
        .chain(["meta", "files", "symbols", "symbols_fts"].iter())
        .any(|name| *name == table);
    if !allowed {
        return Err(StoreError::UnknownTable);
    }
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |row| row.get(0))?)
}
