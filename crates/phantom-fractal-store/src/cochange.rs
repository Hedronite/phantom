//! Co-change history I/O (SPEC-w8 G1b). One git scan's whole result
//! (`CochangeScan`) replaces `git_scan`, `file_history`, `cochange` and
//! the `meta cochange_head` key in one transaction. Every read answers
//! the stored rows with bound statements only; no author, email or
//! message text enters the store, and nothing here writes to the repo
//! tree or its git directory.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::StoreError;

/// The whole mine result of one git scan, as [`write_cochange`] replaces
/// the scan with. Oids stay text and times unix seconds; no author,
/// email or message text enters the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CochangeScan {
    /// HEAD at the scan, or nothing when the scan found no git root.
    pub last_oid: Option<String>,
    /// The oldest commit the window kept, or nothing.
    pub oldest_oid: Option<String>,
    /// How many commits the window walked, merge commits included.
    pub commits_seen: u64,
    /// How many commits were skipped as bulk (over the kept-path cap).
    pub skipped_bulk: u64,
    /// The canonical serialized window.
    pub window_json: String,
    /// One row per historical file, in any input order.
    pub files: Vec<FileRow>,
    /// One kept pair, named by the two end paths in any input order. The
    /// store writes each pair once as `a < b` by the current file row
    /// ids; a pair whose end answers no files row drops at write time.
    pub pairs: Vec<PairRow>,
}

/// One historical file of the scan.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    /// The path the history reached, followed through renames.
    pub path: String,
    /// The sum of the per-commit decay weights.
    pub commits: f64,
    /// The last time this path changed, unix seconds.
    pub last_time: i64,
    /// The path the file came from, when the scan saw a rename.
    pub renamed_from: Option<String>,
}

/// One file pair after canon §7.2's weights, named by the two end paths.
#[derive(Debug, Clone, PartialEq)]
pub struct PairRow {
    /// The lower end's path.
    pub a: String,
    /// The higher end's path.
    pub b: String,
    /// Together weight of the pair.
    pub together: f64,
    /// Appearances of the file `a` alone, and of the file `b` alone.
    pub n_a: f64,
    pub n_b: f64,
    /// Lift of the pair.
    pub lift: f64,
    /// Confidence the file `a` follows `b`, and vice versa.
    pub conf_ab: f64,
    pub conf_ba: f64,
    /// The last commit hash the pair appeared in, as text.
    pub last_oid: String,
    /// The last commit time the pair appeared in, unix seconds.
    pub last_time: i64,
}

/// One partner of [`Store::cochange_of`], with the orientation resolved:
/// the confidence names this end toward the partner and back.
#[derive(Debug, Clone, PartialEq)]
pub struct CochangePartner {
    /// The other end's current path. `None` when the partner end has
    /// left the files table since the scan.
    pub path: Option<String>,
    /// Together weight of the pair.
    pub together: f64,
    /// Appearances of the queried file alone, and of the partner alone.
    pub n_of: f64,
    pub n_other: f64,
    /// Lift of the pair.
    pub lift: f64,
    /// Confidence of this end toward the partner, and back.
    pub conf_of: f64,
    pub conf_to: f64,
    /// The last commit time the pair appeared in, unix seconds.
    pub last_time: i64,
}

/// One file group of [`Store::cochange_partners_for_files`].
#[derive(Debug, Clone, PartialEq)]
pub struct CochangeGroup {
    /// The queried path.
    pub path: String,
    /// Up to `per_file` partners.
    pub partners: Vec<CochangePartner>,
}

/// Every indexed path's file id, in one bound read. The map is built
/// once per `write_cochange` so each pair insert binds numbers only.
fn file_ids(conn: &Connection) -> Result<HashMap<String, i64>, StoreError> {
    let mut statement = conn.prepare("SELECT path, id FROM files")?;
    let mut rows = statement.query([])?;
    let mut map: HashMap<String, i64> = HashMap::new();
    while let Some(row) = rows.next()? {
        map.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
    }
    Ok(map)
}

impl crate::Store {
    /// Replaces the whole co-change scan in one transaction: the
    /// `git_scan` row, every `file_history` row, `cochange` with pairs
    /// stored once as `a < b` by the current file row ids, and the
    /// `meta cochange_head` key set to the scan's HEAD. A pair whose end
    /// answers no files row drops at write time; the next rewritten
    /// scan rebuilds it. No migration runs.
    pub fn write_cochange(&mut self, scan: &CochangeScan) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM git_scan", [])?;
        tx.execute("DELETE FROM file_history", [])?;
        tx.execute("DELETE FROM cochange", [])?;
        tx.execute(
            "INSERT INTO git_scan(id, head_oid, oldest_oid, commits_seen, skipped_bulk, window_json)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)",
            (
                &scan.last_oid,
                &scan.oldest_oid,
                scan.commits_seen as i64,
                scan.skipped_bulk as i64,
                &scan.window_json,
            ),
        )?;
        {
            let ids = file_ids(&tx)?;
            let mut files = tx.prepare(
                "INSERT INTO file_history(path, file_id, commits, last_time, renamed_from)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for file in &scan.files {
                files.execute((
                    &file.path,
                    ids.get(&file.path).copied(),
                    file.commits,
                    file.last_time,
                    &file.renamed_from,
                ))?;
            }
            let mut pairs = tx.prepare(
                "INSERT INTO cochange(
                     a, b, together, n_a, n_b, lift, conf_ab, conf_ba, last_oid, last_time
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for pair in &scan.pairs {
                let (Some(a_id), Some(b_id)) =
                    (ids.get(&pair.a).copied(), ids.get(&pair.b).copied())
                else {
                    continue;
                };
                if a_id == b_id {
                    continue;
                }
                let (a, b, conf_ab, conf_ba) = if a_id < b_id {
                    (a_id, b_id, pair.conf_ab, pair.conf_ba)
                } else {
                    (b_id, a_id, pair.conf_ba, pair.conf_ab)
                };
                pairs.execute((
                    a,
                    b,
                    pair.together,
                    pair.n_a,
                    pair.n_b,
                    pair.lift,
                    conf_ab,
                    conf_ba,
                    &pair.last_oid,
                    pair.last_time,
                ))?;
            }
        }
        // The head stamps the scan: one with a head sets it, and a
        // scan with none — a non-git root — clears it instead of
        // leaving the last git scan's head stale (the W8 Later nit).
        if let Some(head) = &scan.last_oid {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES ('cochange_head', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [head],
            )?;
        } else {
            tx.execute("DELETE FROM meta WHERE key = 'cochange_head'", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The stored scan, swapping each pair end back through the current
    /// files table. Answers nothing when no scan has landed.
    pub fn cochange_scan(&self) -> Result<Option<CochangeScan>, StoreError> {
        let (last_oid, oldest_oid, commits_seen, skipped_bulk, window_json) =
            match read_scan_row(&self.conn)? {
                Some(row) => row,
                None => return Ok(None),
            };
        let files = read_scan_files(&self.conn)?;
        let pairs = read_scan_pairs(&self.conn)?;
        Ok(Some(CochangeScan {
            last_oid,
            oldest_oid,
            commits_seen: commits_seen.unsigned_abs(),
            skipped_bulk: skipped_bulk.unsigned_abs(),
            window_json,
            files,
            pairs,
        }))
    }

    /// The stored head oid from `meta cochange_head`, or nothing (also
    /// nothing when the scan carried no head).
    pub fn cochange_head(&self) -> Result<Option<String>, StoreError> {
        Ok(crate::schema::meta_get(&self.conn, "cochange_head")?.filter(|text| !text.is_empty()))
    }

    /// Partners of one file path, bounded by `limit`, ordered by lift
    /// descending, then together descending, then the partner path. A
    /// path with no file row answers an empty partner list.
    pub fn cochange_of(
        &self,
        path: &str,
        min_lift: f64,
        limit: u32,
    ) -> Result<Vec<CochangePartner>, StoreError> {
        let file_id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |row| {
                row.get(0)
            })
            .ok();
        let Some(file_id) = file_id else {
            return Ok(Vec::new());
        };
        let mut statement = self.conn.prepare(
            "SELECT s.path,
                    CASE WHEN c.a = ?1 THEN c.n_a ELSE c.n_b END,
                    CASE WHEN c.a = ?1 THEN c.n_b ELSE c.n_a END,
                    CASE WHEN c.a = ?1 THEN c.conf_ab ELSE c.conf_ba END,
                    CASE WHEN c.a = ?1 THEN c.conf_ba ELSE c.conf_ab END,
                    c.together, c.lift, c.last_time
             FROM cochange c
             JOIN files s ON s.id = (CASE WHEN c.a = ?1 THEN c.b ELSE c.a END)
             WHERE (c.a = ?1 OR c.b = ?1) AND c.lift >= ?2
             ORDER BY c.lift DESC, c.together DESC, s.path ASC
             LIMIT ?3",
        )?;
        let mut rows = statement.query(rusqlite::params![file_id, min_lift, i64::from(limit)])?;
        let mut out: Vec<CochangePartner> = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(CochangePartner {
                path: row.get(0)?,
                n_of: row.get(1)?,
                n_other: row.get(2)?,
                conf_of: row.get(3)?,
                conf_to: row.get(4)?,
                together: row.get(5)?,
                lift: row.get(6)?,
                last_time: row.get(7)?,
            });
        }
        Ok(out)
    }

    /// Partners of each requested path, `per_file` per path, in input
    /// order, each distinct path read once. No path is skipped for being
    /// missing; it answers an empty partner list.
    pub fn cochange_partners_for_files(
        &self,
        paths: &[String],
        min_lift: f64,
        per_file: u32,
    ) -> Result<Vec<CochangeGroup>, StoreError> {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut out: Vec<CochangeGroup> = Vec::new();
        for path in paths {
            if seen.insert(path.clone()) {
                out.push(CochangeGroup {
                    path: path.clone(),
                    partners: self.cochange_of(path, min_lift, per_file)?,
                });
            }
        }
        Ok(out)
    }
}

/// The stored `git_scan` row, or nothing when no scan has landed.
type ScanRow = (Option<String>, Option<String>, i64, i64, String);

fn read_scan_row(conn: &Connection) -> Result<Option<ScanRow>, StoreError> {
    let row = match conn.query_row(
        "SELECT head_oid, oldest_oid, commits_seen, skipped_bulk, window_json
         FROM git_scan WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    ) {
        Ok(found) => found,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(other) => return Err(other.into()),
    };
    Ok(Some(row))
}

/// Reads the history rows, joining the current file id where the index
/// still holds the path.
fn read_scan_files(conn: &Connection) -> Result<Vec<FileRow>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT h.path, s.id, h.commits, h.last_time, h.renamed_from
         FROM file_history h
         LEFT JOIN files s ON s.path = h.path
         ORDER BY h.path",
    )?;
    let mut rows = statement.query([])?;
    let mut out: Vec<FileRow> = Vec::new();
    while let Some(row) = rows.next()? {
        let _unused_file_id: Option<i64> = row.get(1)?;
        out.push(FileRow {
            path: row.get(0)?,
            commits: row.get(2)?,
            last_time: row.get(3)?,
            renamed_from: row.get(4)?,
        });
    }
    Ok(out)
}

/// Reads the pair rows, swapping the stored ids back through the
/// current files table; a pair whose end left the index answers neither
/// a path nor a row.
fn read_scan_pairs(conn: &Connection) -> Result<Vec<PairRow>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT fa.path, fb.path, c.together, c.n_a, c.n_b, c.lift,
                c.conf_ab, c.conf_ba, c.last_oid, c.last_time
         FROM cochange c
         JOIN files fa ON fa.id = c.a
         JOIN files fb ON fb.id = c.b
         ORDER BY c.a, c.b",
    )?;
    let mut rows = statement.query([])?;
    let mut out: Vec<PairRow> = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(PairRow {
            a: row.get(0)?,
            b: row.get(1)?,
            together: row.get(2)?,
            n_a: row.get(3)?,
            n_b: row.get(4)?,
            lift: row.get(5)?,
            conf_ab: row.get(6)?,
            conf_ba: row.get(7)?,
            last_oid: row.get(8)?,
            last_time: row.get(9)?,
        });
    }
    Ok(out)
}
