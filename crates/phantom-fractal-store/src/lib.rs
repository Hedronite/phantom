#![forbid(unsafe_code)]

//! SQLite store for one Phantom extract.
//!
//! A new database gets the canon §6.3 tables, `producer = phantom-fractal`,
//! and `user_version` equal to [`SCHEMA_VERSION`]. Opening a database with
//! no producer row fails. A `schema_version` newer than this binary fails
//! too. Symbol rows come from [`Language::extract`]; this crate does not
//! invent them.

mod schema;

use std::path::Path;

use phantom_fractal_lang::{Extract, ExtractError, Item, ItemKind, Language, Visibility};
use rusqlite::Connection;

/// Producer stamp written into `meta`.
pub const PRODUCER: &str = "phantom-fractal";

/// Schema this binary creates and will open. A higher `meta.schema_version`
/// is refused.
pub const SCHEMA_VERSION: i64 = 1;

/// Open failure, or a write that could not be stored.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite returned an error.
    Sqlite(rusqlite::Error),
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
    /// The extractor failed before any row was written.
    Extract(ExtractError),
    /// A stored kind is not one this slice writes.
    UnknownKind(String),
    /// A stored visibility is not one this slice writes.
    UnknownVisibility(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(err) => write!(f, "sqlite: {err}"),
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
            Self::Extract(err) => write!(f, "extract: {err}"),
            Self::UnknownKind(kind) => write!(f, "unknown item kind {kind}"),
            Self::UnknownVisibility(text) => write!(f, "unknown visibility {text}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(err) => Some(err),
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
    /// Opens `path`, creating the schema when the database has no `meta` table.
    ///
    /// A database that already has `meta` and no `producer` row is left
    /// unchanged and returns [`StoreError::MissingProducer`].
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    fn from_connection(mut conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        if !schema::meta_table_exists(&conn)? {
            conn.execute_batch("PRAGMA journal_mode = WAL;")?;
            let tx = conn.transaction()?;
            schema::create(&tx)?;
            tx.commit()?;
            return Ok(Self { conn });
        }
        schema::check_stamp(&conn)?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        Ok(Self { conn })
    }

    /// Extracts `source` with `language` and writes the file row plus one
    /// symbol row per extracted item.
    pub fn put_source(
        &mut self,
        path: &str,
        source: &str,
        mtime_ns: i64,
        language: &impl Language,
    ) -> Result<(), StoreError> {
        let extract = language.extract(source).map_err(StoreError::Extract)?;
        self.insert_extract(path, source, mtime_ns, language.name(), &extract)
    }

    /// Items previously stored for `path`, in insert order.
    pub fn read_items(&self, path: &str) -> Result<Vec<Item>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT symbols.kind, symbols.name, symbols.visibility, symbols.start_line
             FROM symbols
             JOIN files ON files.id = symbols.file_id
             WHERE files.path = ?1
             ORDER BY symbols.id",
        )?;
        let rows = statement.query_map([path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        rows.map(|row| row.map_err(StoreError::from).and_then(item_from_row))
            .collect()
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

    fn insert_extract(
        &mut self,
        path: &str,
        source: &str,
        mtime_ns: i64,
        lang: &str,
        extract: &Extract,
    ) -> Result<(), StoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let file_id = insert_file(&tx, path, source, mtime_ns, lang)?;
        insert_symbols(&tx, file_id, path, &extract.items)?;
        tx.commit()?;
        Ok(())
    }
}

fn insert_file(
    conn: &Connection,
    path: &str,
    source: &str,
    mtime_ns: i64,
    lang: &str,
) -> Result<i64, StoreError> {
    let hash = blake3::hash(source.as_bytes());
    let size = i64::try_from(source.len()).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO files(path, lang, blake3, size, mtime_ns, generated, parse_status)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 'ok')",
        (path, lang, hash.as_bytes().as_slice(), size, mtime_ns),
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_symbols(
    conn: &Connection,
    file_id: i64,
    path: &str,
    items: &[Item],
) -> Result<(), StoreError> {
    let mut statement = conn.prepare(
        "INSERT INTO symbols(
            file_id, kind, name, qual_name, stable_key, visibility, start_line
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (index, item) in items.iter().enumerate() {
        let ordinal = items[..index]
            .iter()
            .filter(|prior| prior.name == item.name && prior.kind == item.kind)
            .count();
        let kind = item.kind.as_str();
        let stable_key = format!("{path}#{}#{kind}#{ordinal}", item.name);
        statement.execute((
            file_id,
            kind,
            &item.name,
            &item.name,
            stable_key,
            visibility_to_text(&item.visibility),
            i64::from(item.start_line),
        ))?;
    }
    Ok(())
}

fn item_from_row(row: (String, String, String, Option<i64>)) -> Result<Item, StoreError> {
    let (kind, name, visibility, start_line) = row;
    let start_line = start_line.ok_or(StoreError::UnreadableStamp)?;
    let start_line = u32::try_from(start_line).map_err(|_| StoreError::UnreadableStamp)?;
    Ok(Item {
        kind: kind_from_text(&kind)?,
        name,
        visibility: visibility_from_text(&visibility)?,
        start_line,
    })
}

fn visibility_to_text(visibility: &Visibility) -> String {
    match visibility {
        Visibility::Private => "private".to_owned(),
        Visibility::Public => "public".to_owned(),
        Visibility::Crate => "crate".to_owned(),
        Visibility::Restricted(text) => format!("restricted:{text}"),
    }
}

fn visibility_from_text(text: &str) -> Result<Visibility, StoreError> {
    match text {
        "private" => Ok(Visibility::Private),
        "public" => Ok(Visibility::Public),
        "crate" => Ok(Visibility::Crate),
        other => match other.strip_prefix("restricted:") {
            Some(rest) => Ok(Visibility::Restricted(rest.to_owned())),
            None => Err(StoreError::UnknownVisibility(other.to_owned())),
        },
    }
}

fn kind_from_text(text: &str) -> Result<ItemKind, StoreError> {
    match text {
        "function" => Ok(ItemKind::Function),
        "struct" => Ok(ItemKind::Struct),
        "enum" => Ok(ItemKind::Enum),
        "trait" => Ok(ItemKind::Trait),
        "module" => Ok(ItemKind::Module),
        other => Err(StoreError::UnknownKind(other.to_owned())),
    }
}

#[cfg(test)]
mod tests;
