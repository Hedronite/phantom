//! Bind-only row helpers for one file's extract: replace, the symbol and
//! ref inserts, and the maps between stored kinds and the contract.
//!
//! The W2 insert path could only append a file's rows. The W3 index must
//! also survive edits and deletions, so the whole replace is one
//! transaction driven from here.

use rusqlite::{Connection, Transaction};

use phantom_fractal_lang::{Caller, Extract, Item, ItemKind, RefKind, Visibility};

use crate::StoreError;

/// One stored file's stat row (W9): the fields the freshness probe
/// compares first, plus the stored blake3 for the mismatch refetch.
#[derive(Debug, Clone, PartialEq)]
pub struct FileStat {
    /// The repo-relative path.
    pub path: String,
    /// The stored file size, bytes.
    pub size: i64,
    /// The stored mtime, nanoseconds since the epoch.
    pub mtime_ns: i64,
    /// The stored blake3 digest of the source.
    pub blake3: Vec<u8>,
}

/// Deletes every stored row that belongs to `path`: the file row — with
/// its symbols and refs by cascade — and every edge whose ref, source, or
/// target belongs to the file. Nothing outlives the symbol it names.
///
/// Returns `Ok(false)` when `path` has no file row.
pub(crate) fn delete_file_rows(conn: &Connection, path: &str) -> Result<bool, StoreError> {
    let file_id = match conn.query_row("SELECT id FROM files WHERE path = ?1", [path], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(id) => id,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
        Err(err) => return Err(StoreError::Sqlite(err)),
    };
    conn.execute(
        "DELETE FROM edges WHERE
             ref_id IN (SELECT id FROM refs WHERE file_id = ?1)
             OR (src_kind = 'symbol' AND src_id IN (SELECT id FROM symbols WHERE file_id = ?1))
             OR (dst_kind = 'symbol' AND dst_id IN (SELECT id FROM symbols WHERE file_id = ?1))",
        [file_id],
    )?;
    conn.execute(
        "UPDATE symbols SET parent_id = NULL
         WHERE file_id <> ?1 AND parent_id IN (SELECT id FROM symbols WHERE file_id = ?1)",
        [file_id],
    )?;
    // `modules.file_id` is NO ACTION on delete: a file's map module row
    // keeps its path but must not keep the dead link.
    conn.execute(
        "UPDATE modules SET file_id = NULL WHERE file_id = ?1",
        [file_id],
    )?;
    conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
    Ok(true)
}

/// Replaces whatever `path` stored with `extract`, all in one
/// transaction: the old file row goes first, so UNIQUE `files.path`
/// cannot fire, and the new rows go in contract order.
pub(crate) fn replace_extract(
    conn: &mut Connection,
    path: &str,
    source: &str,
    mtime_ns: i64,
    lang_name: &str,
    extract: &Extract,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    delete_file_rows(&tx, path)?;
    let file_id = insert_file(&tx, path, source, mtime_ns, lang_name)?;
    let index = insert_symbols(&tx, file_id, path, extract)?;
    insert_uses(&tx, file_id, extract)?;
    insert_calls(&tx, file_id, extract, &index)?;
    tx.commit()?;
    Ok(())
}

/// Inserts the file row for one fresh extract and returns its id.
pub(crate) fn insert_file(
    conn: &Transaction<'_>,
    path: &str,
    source: &str,
    mtime_ns: i64,
    lang_name: &str,
) -> Result<i64, StoreError> {
    let hash = blake3::hash(source.as_bytes());
    let size = i64::try_from(source.len()).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO files(path, lang, blake3, size, mtime_ns, generated, parse_status)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, 'ok')",
        (path, lang_name, hash.as_bytes().as_slice(), size, mtime_ns),
    )?;
    Ok(conn.last_insert_rowid())
}

/// The row ids behind [`Caller`] lookups: `(owner, name, start_line, id)`
/// tuples in insert order. Items carry `None` as owner; methods their
/// `impl` owner.
pub(crate) struct SymbolIndex {
    entries: Vec<(Option<String>, String, i64, i64)>,
}

impl SymbolIndex {
    /// The enclosing symbol row for `caller`: `name`, `owner`, and
    /// `start_line` must all match. No fallback — a trait default body,
    /// a nested inner fn, or a `#[cfg]` sibling never borrows another
    /// declaration's row, and no exact match means nothing, not a
    /// same-named neighbour.
    pub(crate) fn id(&self, caller: &Option<Caller>) -> Option<i64> {
        let caller = caller.as_ref()?;
        self.entries
            .iter()
            .find(|(owner, name, start_line, _)| {
                name == &caller.name
                    && owner.as_deref() == caller.owner.as_deref()
                    && *start_line == i64::from(caller.start_line)
            })
            .map(|(_, _, _, id)| *id)
    }
}

/// Inserts one symbol row per item and one per method, items first so
/// [`crate::Store::read_items`] keeps the W2 order. Returns the
/// [`SymbolIndex`] for the ref inserts.
pub(crate) fn insert_symbols(
    conn: &Transaction<'_>,
    file_id: i64,
    path: &str,
    extract: &Extract,
) -> Result<SymbolIndex, StoreError> {
    let mut index = SymbolIndex {
        entries: Vec::with_capacity(extract.items.len() + extract.methods.len()),
    };
    let mut item_statement = conn.prepare(
        "INSERT INTO symbols(
            file_id, kind, name, qual_name, stable_key, visibility, start_line, end_line,
            signature, doc_summary, scope, is_test
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for (position, item) in extract.items.iter().enumerate() {
        let ordinal = extract.items[..position]
            .iter()
            .filter(|prior| prior.name == item.name && prior.kind == item.kind)
            .count();
        let scope = extract
            .item_scopes
            .get(position)
            .ok_or(StoreError::UnreadableStamp)?;
        let qual = qualify(scope, &item.name);
        let stable_key = format!("{path}#{}#{}#{ordinal}", item.name, symbol_text(item.kind));
        item_statement.execute((
            file_id,
            symbol_text(item.kind),
            &item.name,
            qual.clone(),
            stable_key,
            visibility_to_text(&item.visibility),
            i64::from(item.start_line),
            i64::from(item.end_line),
            &item.signature,
            &item.doc_summary,
            scope.clone(),
            i64::from(item.is_test),
        ))?;
        index.entries.push((
            None,
            item.name.clone(),
            i64::from(item.start_line),
            conn.last_insert_rowid(),
        ));
    }
    let mut method_statement = conn.prepare(
        "INSERT INTO symbols(
            file_id, kind, name, qual_name, stable_key, visibility, start_line, end_line,
            signature, doc_summary, scope, owner, owner_path, trait_name, is_test, module_id
         ) VALUES (?1, 'method', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL)",
    )?;
    for (position, method) in extract.methods.iter().enumerate() {
        let ordinal = extract.methods[..position]
            .iter()
            .filter(|prior| prior.name == method.name)
            .count();
        let qual = qualify(&method.scope, &format!("{}::{}", method.owner, method.name));
        let stable_key = format!("{path}#{}::{}#method#{ordinal}", method.owner, method.name);
        method_statement.execute((
            file_id,
            &method.name,
            qual.clone(),
            stable_key,
            visibility_to_text(&method.visibility),
            i64::from(method.start_line),
            i64::from(method.end_line),
            &method.signature,
            &method.doc_summary,
            method.scope.clone(),
            &method.owner,
            &method.owner_path,
            &method.trait_name,
            i64::from(method.is_test),
        ))?;
        index.entries.push((
            Some(method.owner.clone()),
            method.name.clone(),
            i64::from(method.start_line),
            conn.last_insert_rowid(),
        ));
    }
    Ok(index)
}

/// Inserts one `use` row per import leaf, with alias, glob, and scope.
pub(crate) fn insert_uses(
    conn: &Transaction<'_>,
    file_id: i64,
    extract: &Extract,
) -> Result<(), StoreError> {
    let mut statement = conn.prepare(
        "INSERT INTO refs(
            file_id, kind, text, last_segment, line, scope, shadowed, alias, glob
         ) VALUES (?1, 'use', ?2, ?3, ?4, ?5, 0, ?6, ?7)",
    )?;
    for (position, import) in extract.imports.iter().enumerate() {
        let scope = extract
            .import_scopes
            .get(position)
            .ok_or(StoreError::UnreadableStamp)?;
        statement.execute((
            file_id,
            &import.path,
            last_segment(&import.path),
            i64::from(import.start_line),
            scope.clone(),
            &import.alias,
            i64::from(import.glob),
        ))?;
    }
    Ok(())
}

/// Inserts one row per call reference. `kind` is `call`, `method_call`,
/// or `macro`; `from_symbol` is the enclosing symbol in the same file, or
/// nothing when the call sits outside any function.
pub(crate) fn insert_calls(
    conn: &Transaction<'_>,
    file_id: i64,
    extract: &Extract,
    index: &SymbolIndex,
) -> Result<(), StoreError> {
    let mut statement = conn.prepare(
        "INSERT INTO refs(
            file_id, from_symbol, kind, text, last_segment, line, col, scope,
            receiver_hint, receiver, shadowed
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?;
    for reference in &extract.refs {
        statement.execute((
            file_id,
            index.id(&reference.from),
            ref_kind_text(reference.kind),
            &reference.text,
            &reference.last_segment,
            i64::from(reference.line),
            i64::from(reference.col),
            &reference.scope,
            &reference.receiver_type,
            &reference.receiver,
            i64::from(reference.shadowed),
        ))?;
    }
    Ok(())
}

/// The last path segment of an import path.
pub(crate) fn last_segment(path: &str) -> &str {
    path.rsplit("::")
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(path)
}

/// An in-file qualified name: `scope::name`, or `name` without a scope.
fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.to_owned()
    } else {
        format!("{scope}::{name}")
    }
}

/// Stored symbol-kind text.
pub(crate) fn symbol_text(kind: ItemKind) -> &'static str {
    kind.as_str()
}

/// Stored ref-kind text.
pub(crate) fn ref_kind_text(kind: RefKind) -> &'static str {
    match kind {
        RefKind::Call => "call",
        RefKind::MethodCall => "method_call",
        RefKind::Macro => "macro",
        RefKind::MacroArg => "macro_arg",
    }
}

/// Stored ref-kind from text.
pub(crate) fn ref_kind_from_text(kind: &str) -> Result<RefKind, StoreError> {
    match kind {
        "call" => Ok(RefKind::Call),
        "method_call" => Ok(RefKind::MethodCall),
        "macro" => Ok(RefKind::Macro),
        "macro_arg" => Ok(RefKind::MacroArg),
        other => Err(StoreError::UnknownKind(other.to_owned())),
    }
}

/// Stored visibility text.
pub(crate) fn visibility_to_text(visibility: &Visibility) -> String {
    match visibility {
        Visibility::Private => "private".to_owned(),
        Visibility::Public => "public".to_owned(),
        Visibility::Crate => "crate".to_owned(),
        Visibility::Restricted(text) => format!("restricted:{text}"),
    }
}

/// Visibility from text.
pub(crate) fn visibility_from_text(text: &str) -> Result<Visibility, StoreError> {
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

/// The columns `read_items` hands to [`item_from_row`].
type ItemColumns = (
    String,
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

/// Stored item from a read of the detail columns.
pub(crate) fn item_from_row(row: ItemColumns) -> Result<Item, StoreError> {
    let (kind, name, visibility, start_line, end_line, signature, doc_summary, is_test) = row;
    let start_line = start_line.ok_or(StoreError::UnreadableStamp)?;
    let start_line = u32::try_from(start_line).map_err(|_| StoreError::UnreadableStamp)?;
    let end_line = end_line.ok_or(StoreError::UnreadableStamp)?;
    let end_line = u32::try_from(end_line).map_err(|_| StoreError::UnreadableStamp)?;
    Ok(Item {
        kind: item_kind_from_text(&kind)?,
        name,
        visibility: visibility_from_text(&visibility)?,
        start_line,
        end_line,
        signature: signature.ok_or(StoreError::UnreadableStamp)?,
        doc_summary,
        is_test: is_test.unwrap_or(0) != 0,
    })
}

/// Stored item kind from text.
pub(crate) fn item_kind_from_text(kind: &str) -> Result<ItemKind, StoreError> {
    match kind {
        "function" => Ok(ItemKind::Function),
        "struct" => Ok(ItemKind::Struct),
        "enum" => Ok(ItemKind::Enum),
        "trait" => Ok(ItemKind::Trait),
        "module" => Ok(ItemKind::Module),
        other => Err(StoreError::UnknownKind(other.to_owned())),
    }
}
