//! Signature, doc summary and end line cases.
//! An inner doc like this one never counts.

pub mod long;
pub mod shapes;

/// Finds the largest item by key. Ties keep the first one met.
///
/// A second paragraph that never shows.
pub fn largest<'a, T, K>(items: &'a [T], key: impl Fn(&T) -> K) -> Option<&'a T>
where
    T: Clone,
    K: Ord,
{
    let mut best: Option<(&'a T, K)> = None;
    for item in items {
        let next = key(item);
        if best.as_ref().is_none_or(|(_, top)| next > *top) {
            best = Some((item, next));
        }
    }
    best.map(|(item, _)| item)
}

/// Opens a session
/// on the named host and port. Retries are capped.
pub(crate) fn connect(
    host: &str,
    port: u16,
    retries: u8,
) -> Result<u16, String> {
    if host.is_empty() || retries == 0 {
        return Err(String::new());
    }
    Ok(port)
}

#[doc = "Set by attribute, which never counts."]
pub const fn zero() -> u32 {
    0
}

// An ordinary comment.
//// Four slashes are not a doc.
fn quiet() {}

/// Kept across the plain comment below.
// A plain note.
#[inline]
fn noted() {}

/// Helpers for the tests, e.g. small ones.
#[cfg(test)]
mod helpers {
    pub fn tiny() -> u8 {
        1
    }
}
