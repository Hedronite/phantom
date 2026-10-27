//! The workspace lib the other packages call.

pub mod util;

pub use util::helper as again;

pub fn f() {
    util::helper();
}

pub struct Thing;

impl Thing {
    pub fn new() -> Self {
        Self::make()
    }

    fn make() -> Self {
        Thing
    }
}

pub fn own() {
    core_lib::f();
}
