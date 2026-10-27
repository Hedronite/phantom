//! Scope, method, and module fixture for the Phantom extract pack
//! (SPEC-w3 G0b). Written for this pack. Not compiled.

use std::fmt;

mod store;
#[path = "other/place.rs"]
pub mod placed;

pub struct Ledger<T>(T);

impl<T> Ledger<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    fn peek(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for crate::Ledger<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ledger")
    }
}

pub mod outer {
    use super::Ledger;

    pub fn make() {}

    mod inner {
        pub(crate) fn deep() {}

        impl super::super::Ledger<u8> {
            pub fn byte() {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds() {
        make_one();
    }
}
