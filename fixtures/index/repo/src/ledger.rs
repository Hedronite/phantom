//! Ledger types for the index walk fixture.

pub struct Ledger {
    entries: Vec<u64>,
}

pub fn open_ledger() -> Ledger {
    Ledger {
        entries: Vec::new(),
    }
}

impl Ledger {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
