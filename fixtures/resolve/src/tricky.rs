//! Three shapes where a function's name and owner alone name the wrong
//! caller. Each call is credited to the function it sits in, or to none.

pub trait Greet {
    fn greet(&self) {
        target(); // no source: a trait default body has no symbol
    }
}

pub struct Guest;

impl Greet for Guest {
    fn greet(&self) {
        other(); // local, from `Guest::greet` only
    }
}

pub fn outer() {
    fn inner() {
        target(); // no source: a nested fn has no symbol
    }
    inner(); // none: shadowed by the nested fn
}

pub fn inner() {
    other(); // local, from the module-level `inner`
}

#[cfg(unix)]
pub fn twin() {
    target(); // local, from the first twin
}

#[cfg(not(unix))]
pub fn twin() {
    other(); // local, from the second twin
}

pub fn target() {}

pub fn other() {}
