//! The lib under test. It does not gain the dev-dependency on `probe`.

pub struct Counter {
    count: u32,
}

impl Counter {
    pub fn new() -> Self {
        Counter { count: 0 }
    }

    pub fn bump(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

/// `probe` is a dev-dependency only, so this call stays external.
pub fn checked(value: u32) -> bool {
    probe::check(value)
}
