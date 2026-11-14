pub struct Ledger {
    total: u32,
}

impl Ledger {
    pub fn open() -> Self {
        Ledger { total: 0 }
    }

    pub fn count() -> u32 {
        0
    }

    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn settle(&self) {}

    pub fn len(&self) -> usize {
        0
    }

    pub fn audit(&self) {
        assert!(self.total() < 100);
    }
}

pub struct Book;

impl Book {
    pub fn new() -> Self {
        Book
    }

    pub fn total(&self) -> u32 {
        0
    }
}
