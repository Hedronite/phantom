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

mod cochange;
mod detail;
mod dirs;
mod dump;
mod graph;
mod impact;
mod lock;
mod meta;
mod rank;
mod rows;
mod schema;

pub use crate::impact::SymbolFacts;

use std::path::Path;

use phantom_fractal_lang::{ExtractError, Item, Language};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};

pub use crate::cochange::{CochangeGroup, CochangePartner, CochangeScan, FileRow, PairRow};
pub use crate::detail::SymbolDetail;
pub use crate::graph::{CallHit, Package, SymbolMatch};
pub use crate::lock::{LockState, WriterLock, lock_state};
pub use crate::rank::{MapFile, MapRow, RankEdge, RankImportEdge, RankSymbolRow};
pub use crate::rows::FileStat;

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
    /// Another writer holds the database's `graph.db.lock` after the
    /// 250 ms window. The CLI maps this to `lock_held`, exit 4.
    LockHeld,
    /// A read-only open cannot proceed without creating or writing a
    /// sidecar the `fract doctor` never writes: no `-wal`, `-shm`, or
    /// journal, no database file. Nothing about the data dir changes.
    ReadOnlyUnavailable,
    /// The caller's own-test-flag list does not match the file's
    /// stored symbol count.
    OwnFlagCount {
        /// Symbols the file stores.
        stored: usize,
        /// Flags the reset was called with.
        flags: usize,
    },
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
            Self::LockHeld => f.write_str("another writer holds the database lock"),
            Self::ReadOnlyUnavailable => {
                f.write_str("read-only open cannot proceed without a sidecar write")
            }
            Self::OwnFlagCount { stored, flags } => {
                write!(
                    f,
                    "{flags} own test flags do not match the file's {stored} symbols"
                )
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
    /// The path a read-only open serves, so [`read_only_still_valid`]
    /// can re-check the database's state after the reads. `None` when
    /// the store was opened for writes, which cannot go stale.
    ///
    /// [`read_only_still_valid`]: Self::read_only_still_valid
    read_only_path: Option<std::path::PathBuf>,
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
        Ok(Self {
            conn,
            read_only_path: None,
        })
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
        Ok(Self {
            conn,
            read_only_path: None,
        })
    }

    /// Opens `path` strictly for reads (W9 G3), for a `fract doctor`
    /// and `fract repos list` that write nothing at all.
    ///
    /// The gate first: when the writer lock is [`held`](lock_state), or
    /// a `-wal` or `-shm` sits beside the database, the open answers
    /// [`StoreError::ReadOnlyUnavailable`] before any SQLite work — the
    /// database is in use and the guarded read must not race it.
    ///
    /// Past the gate the open takes `SQLITE_OPEN_READ_ONLY` over the
    /// percent-encoded `file:` URI with `mode=ro&immutable=1` (PM
    /// ruling 2026-10-30: the URI ban was the open contract's own, so
    /// `immutable` reads the checkpointed database without locks, a
    /// sidecar, or any write). No file is created — not a `-wal`, a
    /// `-shm`, or a journal, and not the database itself — and no
    /// directory is touched: no `0700` chain work, no owner probe, no
    /// chmod, no migration, no `PRAGMA` that writes.
    ///
    /// The producer stamp and `schema_version` are checked as
    /// [`Store::open`]'s, with the same verdicts: a stamp newer than
    /// [`SCHEMA_VERSION`] refuses via the header preflight before
    /// SQLite runs at all, and a stamp older than it cannot migrate
    /// here, so it is [`StoreError::SchemaMismatch`]. The getters —
    /// [`root`], [`index_head`], [`row_count`], [`file_stats`],
    /// [`workspace_fingerprint`], and the epochs — all answer without a
    /// write; a write attempt answers the SQLite refusal.
    ///
    /// **Callers must re-check after the reads:** the guarded snapshot
    /// only binds while nothing else touched the index.
    /// [`read_only_still_valid`] carries the same two-condition check
    /// the gate made — an unlocked database and no sidecars — and a
    /// `false` there means the reads raced a writer.
    ///
    /// [`root`]: Self::root
    /// [`index_head`]: Self::index_head
    /// [`row_count`]: Self::row_count
    /// [`file_stats`]: Self::file_stats
    /// [`workspace_fingerprint`]: Self::workspace_fingerprint
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        if matches!(lock_state(path), Ok(LockState::Held)) || sidecar_present(path) {
            return Err(StoreError::ReadOnlyUnavailable);
        }
        // The stamp preflight reads the database header directly,
        // without SQLite: a stamp newer than this binary refuses here,
        // so a read-only open of a schema-newer database touches
        // nothing at all.
        if let Some(user_version) = header_user_version(path)
            && user_version > SCHEMA_VERSION
        {
            return Err(StoreError::SchemaNewer {
                found: user_version,
            });
        }
        let conn = Connection::open_with_flags(read_only_uri(path).as_str(), read_only_flags())
            .map_err(read_only_open_error)?;
        // The W2 hardening on the connection only: `PRAGMA
        // foreign_keys = ON`, `PRAGMA trusted_schema = OFF`, the
        // `DEFENSIVE` config. None writes the file.
        #[cfg(test)]
        open_trace::record("read_only_harden");
        harden(&conn)?;
        if !read_only_probe(schema::meta_table_exists(&conn))? {
            return Err(StoreError::ForeignDatabase);
        }
        let version = read_only_probe(schema::check_stamp(&conn))?;
        if version != SCHEMA_VERSION {
            // A stamp this binary opens for writes migrates; a
            // read-only open refuses the stamp instead.
            let user_version = read_only_probe(schema::user_version(&conn))?;
            return Err(StoreError::SchemaMismatch {
                schema_version: version,
                user_version,
            });
        }
        Ok(Self {
            conn,
            read_only_path: Some(path.to_path_buf()),
        })
    }

    /// Re-checks the guard a read-only open ran before it opened: the
    /// database's writer lock is free and no `-wal` or `-shm` sits
    /// beside it. `Ok(false)` means the reads may have raced a
    /// writer — the caller answers [`StoreError::ReadOnlyUnavailable`]
    /// for them. A store opened for writes is never stale: it answers
    /// `Ok(true)`.
    pub fn read_only_still_valid(&self) -> Result<bool, StoreError> {
        let Some(path) = &self.read_only_path else {
            return Ok(true);
        };
        if lock_state(path)? == LockState::Held || sidecar_present(path) {
            return Ok(false);
        }
        Ok(true)
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

    /// One stat row per stored file, sorted by path: the size and
    /// mtime the freshness probe compares, plus the stored blake3.
    pub fn file_stats(&self) -> Result<Vec<rows::FileStat>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT path, size, mtime_ns, blake3 FROM files ORDER BY path")?;
        let stat = |row: &rusqlite::Row<'_>| {
            Ok(rows::FileStat {
                path: row.get(0)?,
                size: row.get(1)?,
                mtime_ns: row.get(2)?,
                blake3: row.get(3)?,
            })
        };
        let rows = statement.query_map([], stat)?;
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
                    symbols.end_line, symbols.signature, symbols.doc_summary, symbols.is_test
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
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
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
/// The database header's `user_version`, read without SQLite. `None`
/// when the file is not a readable SQLite-form file — non-headers fall
/// through to the stamped SQL checks.
fn header_user_version(path: &Path) -> Option<i64> {
    use std::io::Read;
    let mut bytes = [0u8; 100];
    std::fs::File::open(path)
        .ok()?
        .read_exact(&mut bytes)
        .ok()?;
    if bytes[..16] != *b"SQLite format 3\x00" {
        return None;
    }
    let version = u32::from_be_bytes(bytes[60..64].try_into().ok()?);
    Some(i64::from(version))
}

/// Percent-encodes an absolute path as one SQLite `file:` URI, with
/// the read-only and immutable parameters appended.
pub(crate) fn read_only_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::from("file:");
    for byte in text.as_bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte))
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out.push_str("?mode=ro&immutable=1");
    out
}

/// Flags for [`Store::open_read_only`]: read without one, open the
/// `file:` URI percent-encoded path, and never create.
pub(crate) fn read_only_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// `<database>-wal` or `<database>-shm` beside the database: the
/// sidecars the guarded read refuses on, and `read_only_still_valid`
/// re-checks.
pub(crate) fn sidecar_path(db_path: &Path, suffix: &str) -> std::path::PathBuf {
    let name = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "graph.db".to_owned());
    let mut name = name;
    name.push('-');
    name.push_str(suffix);
    match db_path.parent() {
        Some(dir) => dir.join(name),
        None => std::path::PathBuf::from(name),
    }
}

/// True when `-wal` or `-shm` sits beside the database.
pub(crate) fn sidecar_present(db_path: &Path) -> bool {
    sidecar_path(db_path, "wal").exists() || sidecar_path(db_path, "shm").exists()
}

/// Maps a SQLite failure out of the read-only open: the failures that
/// say a read cannot proceed without creating or writing a sidecar —
/// or without a file at all — read as [`StoreError::ReadOnlyUnavailable`];
/// anything else carries the SQLite error through.
fn read_only_open_error(err: rusqlite::Error) -> StoreError {
    if let Some(
        code @ (rusqlite::ffi::ErrorCode::ReadOnly
        | rusqlite::ffi::ErrorCode::CannotOpen
        | rusqlite::ffi::ErrorCode::PermissionDenied),
    ) = err.sqlite_error_code()
    {
        let _ = code;
        return StoreError::ReadOnlyUnavailable;
    }
    StoreError::Sqlite(err)
}

/// Cooler down wrapping: a stamp verdict keeps its kind, a SQLite
/// failure maps like the one from the open itself.
fn read_only_probe<T>(probe: Result<T, StoreError>) -> Result<T, StoreError> {
    match probe {
        Ok(value) => Ok(value),
        Err(StoreError::Sqlite(err)) => Err(read_only_open_error(err)),
        Err(other) => Err(other),
    }
}

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
