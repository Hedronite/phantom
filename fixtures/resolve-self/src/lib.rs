//! Ground truth for `self.m()` and `Self::m()` edges (SPEC-w4 G1d).
//! Indexed, never built.

mod shapes;
mod twins;

use std::fmt;

pub struct Counter {
    value: u32,
    inner: Inner,
}

pub struct Inner;

impl Inner {
    pub fn tick(&self) {}
}

impl Counter {
    pub fn new() -> Self {
        Self::helper();
        Self {
            value: 0,
            inner: Inner,
        }
    }

    fn helper() {}

    pub fn bump(&mut self) {
        self.value += 1;
        self.check();
        self.describe();
        self.inner.tick();
        let other = Inner;
        other.tick();
        let _again = Self::new();
    }

    fn check(&self) {}

    pub fn both(&self) {}
}

pub trait Describe {
    fn describe(&self);

    fn both(&self);

    fn provided(&self) {
        self.describe();
    }
}

impl Describe for Counter {
    fn describe(&self) {
        self.check();
    }

    fn both(&self) {}
}

impl Counter {
    pub fn unclear(&self) {
        self.both();
        self.provided();
        (|| self.check())();
    }
}

impl fmt::Display for Vec<u8> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.check();
        Ok(())
    }
}

pub struct Wrap<T>(T);

impl<T> Wrap<T> {
    fn get(&self) -> &T {
        &self.0
    }

    pub fn peek(&self) {
        self.get();
    }
}

pub fn entry() {
    let counter = Counter::new();
    Counter::new().bump();
    counter.check();
}
