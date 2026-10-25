//! Item fixture for the Phantom extract pack (SPEC-extract G2a).
//! Written for this pack. Not compiled.

pub fn open_ledger(path: &str) -> usize {
    fn count_only() -> usize {
        0
    }
    path.len() + count_only()
}

pub(crate) async fn settle() {}

const unsafe fn quiet() -> u8 {
    0
}

/// A plain record.
#[derive(Debug, Clone)]
pub struct Ledger {
    entries: Vec<u64>,
}

struct Marker;

pub enum Shape {
    Point,
    Line { len: u32 },
}

enum Never {}

pub trait Measure {
    fn measure(&self) -> u32;

    fn twice(&self) -> u32 {
        self.measure() * 2
    }
}

impl Measure for Ledger {
    fn measure(&self) -> u32 {
        self.entries.len() as u32
    }
}

mod outer {
    pub(super) struct Pair(u8, u8);

    pub(in crate::outer) trait Tally {}

    pub(self) fn inner_helper() {}
}
