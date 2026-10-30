//! The canonical dump (W9 D6): every canon table except `index_runs`,
//! with rowids and autoincrement ids replaced by the natural keys they
//! point at — a path for files, a stable key for symbols — each
//! table's rows sorted, so the same graph always gives the same bytes.
//!
//! The serialization is line based and self describing. A `# table`
//! line names the table, a `# cols` line names the columns, and each
//! row is one line of tab-separated cells:
//!
//! - `NULL` encodes as `\N`; no TEXT cell ever starts with a lone
//!   backslash, because text escapes the backslash itself first;
//! - INTEGER encodes in decimal;
//! - REAL encodes as Rust's shortest round-trip decimal (`NaN` and the
//!   infinities keep Rust's spellings);
//! - TEXT escapes the backslash, the very separators the dump uses
//!   (`\t`, `\n`, `\r`), and every other control character as `\xHH`
//!   in lowercase hex;
//! - BLOB encodes as `0x` plus lowercase hex.
//!
//! Composite natural keys — a module or a target is named by several
//! columns — join their parts with a raw `\x1f`, a control character
//! escaped TEXT never emits raw. A referenced id whose target row is
//! gone encodes its cell as `\N`. `cochange` pairs re-orient from the
//! stored lower-id and higher-id ends to the alphabetical path ends,
//! swapping the per-end columns with them, so one pair reads the same
//! whichever build order assigned the file ids.

use std::collections::HashMap;

use rusqlite::Connection;
use rusqlite::types::Value as SqlValue;

use crate::StoreError;

impl crate::Store {
    /// The canonical dump (W9 D6): every table except `index_runs` in a
    /// fixed order, ids replaced by natural keys, rows sorted. The same
    /// graph always gives the same bytes, whichever build order made it.
    pub fn canonical_dump(&self) -> Result<String, StoreError> {
        canonical_dump(&self.conn)
    }
}

/// Every canon table the dump answers, in the dump's fixed order and
/// with its emitted columns. Id columns emit the natural key, so a
/// column's dump name names its role, not its stored type.
const TABLES: &[(&str, &[&str])] = &[
    (
        "cochange",
        &[
            "a",
            "b",
            "together",
            "n_a",
            "n_b",
            "lift",
            "conf_ab",
            "conf_ba",
            "last_oid",
            "last_time",
        ],
    ),
    (
        "edges",
        &[
            "kind",
            "src_kind",
            "src",
            "dst_kind",
            "dst",
            "confidence",
            "reason",
            "ref",
        ],
    ),
    (
        "file_history",
        &["path", "file", "commits", "last_time", "renamed_from"],
    ),
    (
        "files",
        &[
            "path",
            "lang",
            "blake3",
            "size",
            "mtime_ns",
            "module",
            "generated",
            "parse_status",
        ],
    ),
    (
        "git_scan",
        &[
            "head_oid",
            "oldest_oid",
            "commits_seen",
            "skipped_bulk",
            "window_json",
        ],
    ),
    ("meta", &["key", "value"]),
    ("modules", &["target", "path", "file", "parent", "scope"]),
    ("packages", &["name", "version", "manifest_path"]),
    ("rank", &["node_kind", "node", "score", "epoch"]),
    (
        "refs",
        &[
            "file",
            "from_symbol",
            "kind",
            "text",
            "last_segment",
            "line",
            "col",
            "receiver_hint",
            "scope",
            "receiver",
            "shadowed",
            "alias",
            "glob",
        ],
    ),
    (
        "symbols",
        &[
            "file",
            "module",
            "parent",
            "kind",
            "name",
            "qual_name",
            "stable_key",
            "visibility",
            "signature",
            "doc_summary",
            "start_byte",
            "end_byte",
            "start_line",
            "end_line",
            "is_test",
            "is_entry",
            "scope",
            "owner",
            "owner_path",
            "trait_name",
        ],
    ),
    (
        "symbols_fts",
        &[
            "symbol_key",
            "name",
            "qual_name",
            "signature",
            "doc_summary",
            "path",
        ],
    ),
    ("targets", &["package", "kind", "name", "root_file"]),
];

/// A referenced id's target is gone, or the cell is absent: `\N`.
const DANGLING: &str = "\\N";

/// Reads a whole statement as raw SQLite values, one row per inner vec.
fn query_values(conn: &Connection, sql: &str) -> Result<Vec<Vec<SqlValue>>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let count = statement.column_count();
    let mut rows = statement.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(count);
        for index in 0..count {
            cells.push(row.get(index)?);
        }
        out.push(cells);
    }
    Ok(out)
}

/// Escapes one TEXT cell for the dump's serialization.
fn encode_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 || u32::from(c) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", u32::from(c)))
            }
            c => out.push(c),
        }
    }
    out
}

/// Encodes one raw cell of the dump.
fn encode_cell(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => DANGLING.to_owned(),
        SqlValue::Integer(number) => number.to_string(),
        SqlValue::Real(number) => format!("{number}"),
        SqlValue::Text(text) => encode_text(text),
        SqlValue::Blob(bytes) => {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut out = String::with_capacity(2 + bytes.len() * 2);
            out.push_str("0x");
            for byte in bytes {
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
            out
        }
    }
}

/// Joins one composite natural key's parts; escaped TEXT never emits
/// the separator raw.
fn tuple_key(parts: &[SqlValue]) -> String {
    parts
        .iter()
        .map(encode_cell)
        .collect::<Vec<_>>()
        .join("\x1f")
}

/// A `(id, text)` pair from a raw two-column row.
fn extract_pair(mut row: Vec<SqlValue>) -> Option<(i64, String)> {
    let text = match row.pop()? {
        SqlValue::Text(text) => text,
        _ => return None,
    };
    match row.pop()? {
        SqlValue::Integer(id) => Some((id, text)),
        _ => None,
    }
}

fn extract_id_text(rows: Vec<Vec<SqlValue>>) -> HashMap<i64, String> {
    rows.into_iter().filter_map(extract_pair).collect()
}

/// The natural key of a referenced id, `\N`-encoded as a cell when the
/// id is `NULL`, absent, or its target is gone.
fn lookup(map: &HashMap<i64, String>, id: &SqlValue) -> SqlValue {
    match id {
        SqlValue::Integer(id) => match map.get(id) {
            Some(text) => SqlValue::Text(text.clone()),
            None => SqlValue::Text(DANGLING.to_owned()),
        },
        _ => SqlValue::Null,
    }
}

/// One id to natural-key map.
type IdMap = HashMap<i64, String>;

/// One statement's raw rows.
type RawRows = Vec<Vec<SqlValue>>;

/// Every id map the dump's tables resolve through, built once per dump.
struct Maps {
    files: HashMap<i64, String>,
    packages: HashMap<i64, String>,
    targets: HashMap<i64, String>,
    modules: HashMap<i64, String>,
    symbols: HashMap<i64, String>,
    refs: HashMap<i64, String>,
}

/// Builds the package→manifest-path map and the targets over it.
fn maps_of_targets(conn: &Connection) -> Result<(IdMap, IdMap), StoreError> {
    let packages = extract_id_text(query_values(
        conn,
        "SELECT id, manifest_path FROM packages",
    )?);
    let mut targets: HashMap<i64, String> = HashMap::new();
    for row in query_values(conn, "SELECT id, package_id, kind, name FROM targets")? {
        let SqlValue::Integer(id) = row[0] else {
            continue;
        };
        let SqlValue::Integer(package) = row[1] else {
            continue;
        };
        let Some(path) = packages.get(&package) else {
            continue;
        };
        if !matches!(&row[2], SqlValue::Text(_)) || !matches!(&row[3], SqlValue::Text(_)) {
            continue;
        }
        let key = tuple_key(&[SqlValue::Text(path.clone()), row[2].clone(), row[3].clone()]);
        targets.insert(id, key);
    }
    Ok((packages, targets))
}

/// Builds the module map: each module's natural key is its target, its
/// path, and its scope — the canon UNIQUE pair plus the inline scope.
fn maps_of_modules(
    conn: &Connection,
    targets: &HashMap<i64, String>,
) -> Result<HashMap<i64, String>, StoreError> {
    let mut modules: HashMap<i64, String> = HashMap::new();
    for row in query_values(conn, "SELECT id, target_id, path, scope FROM modules")? {
        let SqlValue::Integer(id) = row[0] else {
            continue;
        };
        let SqlValue::Integer(target) = row[1] else {
            continue;
        };
        let Some(target_key) = targets.get(&target) else {
            continue;
        };
        let empty = String::new();
        let path = match &row[2] {
            SqlValue::Text(path) => path,
            _ => &empty,
        };
        let scope = match &row[3] {
            SqlValue::Text(scope) => scope,
            _ => &empty,
        };
        modules.insert(
            id,
            tuple_key(&[
                SqlValue::Text(target_key.clone()),
                SqlValue::Text(path.clone()),
                SqlValue::Text(scope.clone()),
            ]),
        );
    }
    Ok(modules)
}

/// Builds the refs table's emitted rows — the stored `id` dropped —
/// plus the map an edge's `ref_id` resolves through. A ref's key is
/// its identifying columns with an ordinal among the rows that share
/// them, taken in rowid order: the rows of one identity group all sit
/// in one file's insert batch, so their relative rowids are stable
/// across build orders.
fn refs_table_and_keys(
    conn: &Connection,
    files: &HashMap<i64, String>,
    symbols: &HashMap<i64, String>,
) -> Result<(RawRows, IdMap), StoreError> {
    let raw = query_values(
        conn,
        "SELECT id, file_id, from_symbol, kind, text, last_segment, line, col,
                receiver_hint, scope, receiver, shadowed, alias, glob
         FROM refs
         ORDER BY id",
    )?;
    let mut keys: HashMap<i64, String> = HashMap::with_capacity(raw.len());
    let mut rows: Vec<Vec<SqlValue>> = Vec::with_capacity(raw.len());
    let mut ordinals: HashMap<String, i64> = HashMap::new();
    for row in &raw {
        let SqlValue::Integer(id) = row[0] else {
            continue;
        };
        let file = lookup(files, &row[1]);
        let from_symbol = lookup(symbols, &row[2]);
        let identity = tuple_key(&[
            file.clone(),
            from_symbol.clone(),
            row[3].clone(),
            row[4].clone(),
            row[5].clone(),
            row[6].clone(),
            row[7].clone(),
            row[8].clone(),
            row[10].clone(),
        ]);
        let ordinal = ordinals
            .entry(identity.clone())
            .and_modify(|seen| *seen += 1)
            .or_insert(0);
        keys.insert(
            id,
            tuple_key(&[
                SqlValue::Text(identity),
                SqlValue::Text(ordinal.to_string()),
            ]),
        );
        rows.push(vec![
            file,
            from_symbol,
            row[3].clone(),
            row[4].clone(),
            row[5].clone(),
            row[6].clone(),
            row[7].clone(),
            row[8].clone(),
            row[9].clone(),
            row[10].clone(),
            row[11].clone(),
            row[12].clone(),
            row[13].clone(),
        ]);
    }
    Ok((rows, keys))
}

/// Resolves one edges or rank node id through the natural-key map its
/// kind names. Unknown kinds resolve `\N`.
fn node_value(kind: &SqlValue, id: &SqlValue, maps: &Maps) -> SqlValue {
    match kind {
        SqlValue::Text(kind) => {
            let map = match kind.as_str() {
                "symbol" => &maps.symbols,
                "module" => &maps.modules,
                "file" => &maps.files,
                "package" => &maps.packages,
                _ => return SqlValue::Null,
            };
            lookup(map, id)
        }
        _ => SqlValue::Null,
    }
}

/// Reworks one table's raw rows into its emitted cells.
fn dump_table(
    conn: &Connection,
    name: &str,
    maps: &Maps,
    ref_rows: &[Vec<SqlValue>],
) -> Result<Vec<Vec<SqlValue>>, StoreError> {
    let rows: Vec<Vec<SqlValue>> = match name {
        "cochange" => {
            let pairs = query_values(
                conn,
                "SELECT fa.path, fb.path, c.together, c.n_a, c.n_b, c.lift,
                        c.conf_ab, c.conf_ba, c.last_oid, c.last_time
                 FROM cochange c
                 JOIN files fa ON fa.id = c.a
                 JOIN files fb ON fb.id = c.b",
            )?;
            pairs
                .into_iter()
                .map(|row| {
                    // Stored `a` names the lower file id; the emitted key
                    // re-orients the ends to the alphabetical paths and
                    // carries the confidence columns over. `n_a`/`n_b`
                    // count the pair's named ends — the scan names the
                    // ends by path — so they ride unswapped.
                    if encode_cell(&row[0]) <= encode_cell(&row[1]) {
                        row
                    } else {
                        vec![
                            row[1].clone(),
                            row[0].clone(),
                            row[2].clone(),
                            row[3].clone(),
                            row[4].clone(),
                            row[5].clone(),
                            row[7].clone(),
                            row[6].clone(),
                            row[8].clone(),
                            row[9].clone(),
                        ]
                    }
                })
                .collect()
        }
        "edges" => {
            let raw = query_values(
                conn,
                "SELECT kind, src_kind, src_id, dst_kind, dst_id, confidence, reason, ref_id
                 FROM edges",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        row[0].clone(),
                        row[1].clone(),
                        node_value(&row[1], &row[2], maps),
                        row[3].clone(),
                        node_value(&row[3], &row[4], maps),
                        row[5].clone(),
                        row[6].clone(),
                        lookup(&maps.refs, &row[7]),
                    ]
                })
                .collect()
        }
        "file_history" => {
            let raw = query_values(
                conn,
                "SELECT path, file_id, commits, last_time, renamed_from
                 FROM file_history",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        row[0].clone(),
                        lookup(&maps.files, &row[1]),
                        row[2].clone(),
                        row[3].clone(),
                        row[4].clone(),
                    ]
                })
                .collect()
        }
        "files" => {
            let raw = query_values(
                conn,
                "SELECT path, lang, blake3, size, mtime_ns, module_id, generated, parse_status
                 FROM files",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        row[0].clone(),
                        row[1].clone(),
                        row[2].clone(),
                        row[3].clone(),
                        row[4].clone(),
                        lookup(&maps.modules, &row[5]),
                        row[6].clone(),
                        row[7].clone(),
                    ]
                })
                .collect()
        }
        "git_scan" => query_values(
            conn,
            "SELECT head_oid, oldest_oid, commits_seen, skipped_bulk, window_json FROM git_scan",
        )?,
        "meta" => query_values(conn, "SELECT key, value FROM meta")?,
        "modules" => {
            let raw = query_values(
                conn,
                "SELECT target_id, path, file_id, parent_id, scope FROM modules",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        lookup(&maps.targets, &row[0]),
                        row[1].clone(),
                        lookup(&maps.files, &row[2]),
                        lookup(&maps.modules, &row[3]),
                        row[4].clone(),
                    ]
                })
                .collect()
        }
        "packages" => query_values(conn, "SELECT name, version, manifest_path FROM packages")?,
        "rank" => {
            let raw = query_values(conn, "SELECT node_kind, node_id, score, epoch FROM rank")?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        row[0].clone(),
                        node_value(&row[0], &row[1], maps),
                        row[2].clone(),
                        row[3].clone(),
                    ]
                })
                .collect()
        }
        "refs" => ref_rows.to_vec(),
        "symbols" => {
            let raw = query_values(
                conn,
                "SELECT file_id, module_id, parent_id, kind, name, qual_name,
                        stable_key, visibility, signature, doc_summary, start_byte,
                        end_byte, start_line, end_line, is_test, is_entry, scope,
                        owner, owner_path, trait_name
                 FROM symbols",
            )?;
            raw.into_iter()
                .map(|row| {
                    let mut out = row.clone();
                    out[0] = lookup(&maps.files, &row[0]);
                    out[1] = lookup(&maps.modules, &row[1]);
                    out[2] = lookup(&maps.symbols, &row[2]);
                    out
                })
                .collect()
        }
        "symbols_fts" => {
            let raw = query_values(
                conn,
                "SELECT rowid, name, qual_name, signature, doc_summary, path FROM symbols_fts",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        lookup(&maps.symbols, &row[0]),
                        row[1].clone(),
                        row[2].clone(),
                        row[3].clone(),
                        row[4].clone(),
                        row[5].clone(),
                    ]
                })
                .collect()
        }
        "targets" => {
            let raw = query_values(
                conn,
                "SELECT package_id, kind, name, root_file FROM targets",
            )?;
            raw.into_iter()
                .map(|row| {
                    vec![
                        lookup(&maps.packages, &row[0]),
                        row[1].clone(),
                        row[2].clone(),
                        row[3].clone(),
                    ]
                })
                .collect()
        }
        // `TABLES` names only canon tables; an unknown name is a bug.
        _ => return Err(StoreError::UnknownTable),
    };
    Ok(rows)
}

/// A one-line row encoding.
fn encode_row(row: &[SqlValue]) -> String {
    row.iter().map(encode_cell).collect::<Vec<_>>().join("\t")
}

/// The whole dump: the id maps, then every table rendered, sorted, and
/// joined.
pub(crate) fn canonical_dump(conn: &Connection) -> Result<String, StoreError> {
    let files = extract_id_text(query_values(conn, "SELECT id, path FROM files")?);
    let symbols = extract_id_text(query_values(conn, "SELECT id, stable_key FROM symbols")?);
    let (packages, targets) = maps_of_targets(conn)?;
    let modules = maps_of_modules(conn, &targets)?;
    let (ref_rows, refs) = refs_table_and_keys(conn, &files, &symbols)?;
    let maps = Maps {
        files,
        packages,
        targets,
        modules,
        symbols,
        refs,
    };
    let mut out = String::new();
    for (name, columns) in TABLES {
        out.push_str("# table ");
        out.push_str(name);
        out.push('\n');
        out.push_str("# cols ");
        out.push_str(
            &columns
                .iter()
                .map(|column| encode_text(column))
                .collect::<Vec<_>>()
                .join("\t"),
        );
        out.push('\n');
        let rows = dump_table(conn, name, &maps, &ref_rows)?;
        let mut lines: Vec<String> = rows.iter().map(|row| encode_row(row)).collect();
        lines.sort_unstable();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(out)
}
