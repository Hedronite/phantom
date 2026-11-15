//! One statement's rows through its mapper, shared by the read modules.
//!
//! A read prepares one statement, queries it, and hands every row to its
//! mapper through [`collect_rows`]; a column the result type does not
//! carry, or a mark the caller owns, is read by the caller and passed in
//! (W13 R14).
use crate::StoreError;
/// Every row of one statement through its mapper, in statement order.
pub(crate) fn collect_rows<T>(
    rows: &mut rusqlite::Rows<'_>,
    map: impl Fn(&rusqlite::Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map(row)?);
    }
    Ok(out)
}
