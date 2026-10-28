#![forbid(unsafe_code)]

//! SQLite store for one Phantom extract.
//!
//! A new database gets the canon §6.3 tables, `producer = phantom-fractal`,
//! and `user_version` equal to [`SCHEMA_VERSION`]. Opening a database with
//! no producer row fails. A `schema_version` newer than this binary fails
//! too. A version-1 or version-2 database migrates additively to
//! [`SCHEMA_VERSION`].
//!
//! `put_source` replaces a path's rows in one transaction, so an edit and
//! a new index cannot collide on `UNIQUE`, and `remove_file` drops a
//! deleted file. The crate-graph API ([`Store::write_crate_graph`],
//! [`Store::resolve_input`], [`Store::replace_edges`], and the caller
//! counters) serves the resolver on top of the same rows.

mod dirs;
mod graph;
mod impact;
mod rows;
mod schema;

pub use crate::impact::SymbolFacts;

use std::path::Path;

use phantom_fractal_lang::{ExtractError, Item, Language};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

pub use crate::graph::{CallHit, Package, SymbolMatch};

/// Producer stamp written into `meta`.
pub const PRODUCER: &str = "phantom-fractal";

/// Schema this binary creates and will open. A higher `meta.schema_version`
/// is refused.
pub const SCHEMA_VERSION: i64 = 3;

/// Open failure, or a write that could not be stored.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite returned an error.
    Sqlite(rusqlite::Error),
    /// Creating the database file failed.
    Io(std::io::Error),
    /// `meta` has no `producer` row.
    MissingProducer,
    /// `producer` is present and is not [`PRODUCER`].
    ForeignProducer,
    /// `schema_version` is newer than [`SCHEMA_VERSION`].
    SchemaNewer {
        /// Value stored in `meta`.
        found: i64,
    },
    /// `user_version` does not equal `meta.schema_version`.
    SchemaMismatch {
        /// Value stored in `meta`.
        schema_version: i64,
        /// `PRAGMA user_version`.
        user_version: i64,
    },
    /// The stamp is present but not a schema this binary understands.
    UnreadableStamp,
    /// An existing SQLite file has no `meta` table, so it is not ours.
    ForeignDatabase,
    /// `row_count` was asked for a table this crate does not expose.
    UnknownTable,
    /// The extractor failed before any row was written.
    Extract(ExtractError),
    /// A stored kind or ref kind is not one this slice writes.
    UnknownKind(String),
    /// A stored visibility is not one this slice writes.
    UnknownVisibility(String),
    /// A stored edge reason is not one this slice writes.
    UnknownReason(String),
    /// A target names a package the crate graph does not carry.
    UnknownPackage(String),
    /// A module names a target the crate graph does not carry.
    UnknownTarget(i64),
    /// `repos/` or the repo directory is a symlink or not a directory.
    DataDirectory(String, String),
    /// A store directory is owned by another uid, or the owner probe
    /// could not be written where the effective uid owns.
    NotOwnedByUs(String, String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(err) => write!(f, "sqlite: {err}"),
            Self::Io(err) => write!(f, "io: {err}"),
            Self::MissingProducer => f.write_str("database has no producer row"),
            Self::ForeignProducer => f.write_str("producer is not phantom-fractal"),
            Self::SchemaNewer { found } => {
                write!(f, "schema_version {found} is newer than this binary")
            }
            Self::SchemaMismatch {
                schema_version,
                user_version,
            } => write!(
                f,
                "user_version {user_version} does not match schema_version {schema_version}"
            ),
            Self::UnreadableStamp => f.write_str("schema stamp is not readable"),
            Self::ForeignDatabase => f.write_str("database is not a phantom-fractal index"),
            Self::UnknownTable => f.write_str("table name is not allowed"),
            Self::Extract(err) => write!(f, "extract: {err}"),
            Self::UnknownKind(kind) => write!(f, "unknown kind {kind}"),
            Self::UnknownVisibility(text) => write!(f, "unknown visibility {text}"),
            Self::UnknownReason(reason) => write!(f, "unknown edge reason {reason}"),
            Self::UnknownPackage(name) => {
                write!(f, "the graph has no package named {name}")
            }
            Self::UnknownTarget(target) => {
                write!(f, "the graph has no target with id {target}")
            }
            Self::DataDirectory(path, detail) => {
                write!(f, "{path}: {detail}")
            }
            Self::NotOwnedByUs(path, detail) => {
                write!(f, "{path}: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::Extract(err) => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

/// One stamped graph database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens `path`.
    ///
    /// A missing path is created with mode `0600` and the canon schema.
    /// Missing directories on the way to it — the data root when that is
    /// new, `repos/`, and `repos/<name>-<hash>/` — are created `0700`.
    /// On open, `repos/` and the repo directory are narrowed to `0700`
    /// when they carry any group or other bit; a data root that already
    /// existed is never changed. A `repos/` or repo directory that is a
    /// symlink or not a directory fails the open before anything is
    /// chmodded, and a chain member owned by another uid fails it too.
    /// An existing SQLite file with no `meta` table, or a `producer` other
    /// than [`PRODUCER`], is left unchanged and returns an error.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        #[cfg(unix)]
        prepare_chain(path)?;
        if path.exists() {
            Self::open_existing(path)
        } else {
            Self::open_new(path)
        }
    }

    fn open_new(path: &Path) -> Result<Self, StoreError> {
        create_private_file(path)?;
        let mut conn = Connection::open_with_flags(path, open_flags(true))?;
        harden(&conn)?;
        enable_wal(&conn)?;
        let tx = conn.transaction()?;
        schema::create(&tx)?;
        tx.commit()?;
        Ok(Self { conn })
    }

    fn open_existing(path: &Path) -> Result<Self, StoreError> {
        let mut conn = Connection::open_with_flags(path, open_flags(false))?;
        harden(&conn)?;
        if !schema::meta_table_exists(&conn)? {
            return Err(StoreError::ForeignDatabase);
        }
        let mut version = schema::check_stamp(&conn)?;
        if version < 2 {
            schema::migrate_v1_to_v2(&mut conn)?;
            version = 2;
        }
        if version < SCHEMA_VERSION {
            schema::migrate_v2_to_v3(&mut conn)?;
        }
        enable_wal(&conn)?;
        Ok(Self { conn })
    }

    /// Extracts `source` with `language` and **replaces** the rows stored
    /// for `path` in one transaction: the file row, its symbols, its use
    /// and call refs, methods as symbols of kind `method`, and every edge
    /// whose ref, source, or target the file owned. A UNIQUE collision on
    /// `files.path` cannot happen.
    pub fn put_source(
        &mut self,
        path: &str,
        source: &str,
        mtime_ns: i64,
        language: &impl Language,
    ) -> Result<(), StoreError> {
        let extract = language.extract(source).map_err(StoreError::Extract)?;
        rows::replace_extract(
            &mut self.conn,
            path,
            source,
            mtime_ns,
            language.name(),
            &extract,
        )
    }

    /// Deletes the rows stored for `path` and the file row, with every
    /// edge that named its symbols. `Ok(true)` when a row was deleted,
    /// `Ok(false)` when `path` had none.
    pub fn remove_file(&mut self, path: &str) -> Result<bool, StoreError> {
        rows::delete_file_rows(&self.conn, path)
    }

    /// Every stored repo-relative path, sorted.
    pub fn file_paths(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self.conn.prepare("SELECT path FROM files ORDER BY path")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn trusted_schema_is_off(&self) -> Result<bool, StoreError> {
        trusted_schema_off(&self.conn)
    }

    pub fn synchronous_is_normal(&self) -> Result<bool, StoreError> {
        synchronous_is_normal(&self.conn)
    }

    pub fn defensive_is_on(&self) -> Result<bool, StoreError> {
        defensive_on(&self.conn)
    }

    /// Items stored for `path`, in insert order. Methods stay out: they
    /// are symbols of kind `method`, not items.
    pub fn read_items(&self, path: &str) -> Result<Vec<Item>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT symbols.kind, symbols.name, symbols.visibility, symbols.start_line,
                    symbols.is_test
             FROM symbols
             JOIN files ON files.id = symbols.file_id
             WHERE files.path = ?1 AND symbols.kind <> 'method'
             ORDER BY symbols.id",
        )?;
        let rows = statement.query_map([path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })?;
        rows.map(|row| row.map_err(StoreError::from).and_then(rows::item_from_row))
            .collect()
    }

    /// Blake3 digest [`Self::put_source`] stored for `path`.
    ///
    /// `None` when that repo-relative path has no file row.
    pub fn stored_blake3(&self, path: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self
            .conn
            .query_row("SELECT blake3 FROM files WHERE path = ?1", [path], |row| {
                row.get(0)
            }) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Texts of `use` refs stored for `path`, in insert order.
    pub fn ref_texts(&self, path: &str) -> Result<Vec<String>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT refs.text
             FROM refs
             JOIN files ON files.id = refs.file_id
             WHERE files.path = ?1 AND refs.kind = 'use'
             ORDER BY refs.id",
        )?;
        let rows = statement.query_map([path], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// How many FTS rows match `query`.
    ///
    /// The statement is [`MATCH_SQL`]. `query` is bound as one phrase, so a
    /// quote inside it cannot change the SQL.
    pub fn match_count(&self, query: &str) -> Result<i64, StoreError> {
        let phrase = phrase_quote(query);
        self.conn
            .query_row(MATCH_SQL, [phrase], |row| row.get(0))
            .map_err(StoreError::from)
    }

    /// `meta.producer`.
    pub fn producer(&self) -> Result<Option<String>, StoreError> {
        schema::meta_get(&self.conn, "producer")
    }

    /// `meta.schema_version` as an integer.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        let raw =
            schema::meta_get(&self.conn, "schema_version")?.ok_or(StoreError::UnreadableStamp)?;
        raw.parse().map_err(|_| StoreError::UnreadableStamp)
    }

    /// `PRAGMA user_version`.
    pub fn user_version(&self) -> Result<i64, StoreError> {
        schema::user_version(&self.conn)
    }

    /// Row count of a canon table this crate creates.
    pub fn row_count(&self, table: &str) -> Result<i64, StoreError> {
        schema::row_count(&self.conn, table)
    }
}

/// Flags for [`Store::open`]. `SQLITE_OPEN_URI` is never set.
pub(crate) fn open_flags(create: bool) -> OpenFlags {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    flags
}

fn create_private_file(path: &Path) -> Result<(), StoreError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(StoreError::Io)?;
    Ok(())
}

/// Runs the W2 `0700` work over the recognized chain: shape refusal
/// first, then creation, then the effective-uid check with a probe file
/// the store itself creates, then the tighten. A plain database with no
/// `repos/` ancestor keeps the W2 behavior alone.
#[cfg(unix)]
fn prepare_chain(path: &Path) -> Result<(), StoreError> {
    if let Some(chain) = dirs::Chain::new(path) {
        dirs::verify_shape(&chain)?;
        dirs::create_private_directories(path)?;
        let probe = chain.repo_dir.join(format!(
            ".owner-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|dur| dur.as_nanos())
                .unwrap_or(0)
        ));
        let effective_uid = dirs::effective_uid(&chain.repo_dir, &probe)?;
        dirs::verify_owned(&chain, effective_uid)?;
        dirs::tighten_directories(&chain, effective_uid)?;
    } else {
        dirs::create_private_directories(path)?;
    }
    Ok(())
}

fn harden(conn: &Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    open_trace::record("harden");
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA trusted_schema = OFF;",
    )?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    Ok(())
}

fn enable_wal(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

/// `PRAGMA trusted_schema` is off.
pub(crate) fn trusted_schema_off(conn: &Connection) -> Result<bool, StoreError> {
    let value: i64 = conn.pragma_query_value(None, "trusted_schema", |row| row.get(0))?;
    Ok(value == 0)
}

/// `PRAGMA synchronous` is `NORMAL` (1).
pub(crate) fn synchronous_is_normal(conn: &Connection) -> Result<bool, StoreError> {
    let value: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    Ok(value == 1)
}

/// `SQLITE_DBCONFIG_DEFENSIVE` is on.
pub(crate) fn defensive_on(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE)?)
}

/// Default index database for `repo`. The path is under the Phantom data
/// directory, not inside `repo`.
pub fn index_database_path(repo: &Path) -> std::path::PathBuf {
    let canonical = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let name = canonical
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or("repo");
    let hash = blake3::hash(canonical.as_os_str().as_encoded_bytes());
    let hash8 = hex8(&hash.as_bytes()[..4]);
    data_home()
        .join("repos")
        .join(format!("{}-{hash8}", slug(name)))
        .join("graph.db")
}

fn data_home() -> std::path::PathBuf {
    if let Some(over) = std::env::var_os("PHANTOM_FRACTAL_DATA") {
        return std::path::PathBuf::from(over);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/phantom-fractal")
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        base.join("phantom-fractal")
    }
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "repo".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn hex8(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(8);
    for byte in bytes.iter().take(4) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Fixed `MATCH` statement. User text is never interpolated.
pub const MATCH_SQL: &str = "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH ?1";

/// Wraps `query` as one FTS phrase and doubles any embedded quote.
pub(crate) fn phrase_quote(query: &str) -> String {
    let mut out = String::from("\"");
    for ch in query.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Ordered record of the open path, test-only (SecOps review #12 N2). Every step
/// between the connection opening and the migration appends its static
/// name; `tests::harden_is_first_on_an_existing_database` fails unless
/// `harden` is recorded before the stamp probe and both migrations.
#[cfg(test)]
pub(crate) mod open_trace {
    use std::cell::RefCell;

    thread_local! {
        static STEPS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(crate) fn record(step: &'static str) {
        STEPS.with(|steps| steps.borrow_mut().push(step));
    }

    pub(crate) fn clear() {
        STEPS.with(|steps| steps.borrow_mut().clear());
    }

    pub(crate) fn steps() -> Vec<&'static str> {
        STEPS.with(|steps| steps.borrow().clone())
    }
}

#[cfg(test)]
mod tests;
