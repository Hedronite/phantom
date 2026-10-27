//! Resolve fixture crate root (SPEC-w3 G2d). Written for this pack. Not
//! compiled. Each call notes the edge it should get, or why it gets none.

mod util;
pub mod shapes;
#[path = "elsewhere/placed.rs"]
mod placed;
#[path = "../../outside.rs"]
mod escaped;

use crate::util::helper;
use shapes::*;
use util::helper as aliased;

pub fn entry() {
    crate::util::helper(); // path-exact
    self::local_fn(); // path-exact
    local_fn(); // local
    util::helper(); // local, through a child module
    helper(); // use-alias
    aliased(); // use-alias, through `as`
    Counter::new(); // assoc: the one inherent `new`
    Counter::tally(); // none: only a trait impl has `tally`
    std::mem::drop(1); // external
    area(); // none: only the glob `shapes::*` supplies it
    twin(); // none: two `#[cfg]` candidates
    placed::from_path(); // local, through a `#[path]` module
    println!("{}", 1); // macro
    Counter::new().bump(); // assoc for `new`, a method call for `bump`
    let shadow = |x: u32| x;
    shadow(2); // none: shadowed by the `let`
    escaped::nothing(); // none: `#[path]` leaves the root
}

pub fn local_fn() {}

#[cfg(unix)]
pub fn twin() {}

#[cfg(not(unix))]
pub fn twin() {}

pub struct Counter;

impl Counter {
    pub fn new() -> Self {
        Counter
    }

    pub fn bump(&self) {}
}

pub trait Make {
    fn new() -> Self;
}

impl Make for Counter {
    fn new() -> Self {
        Counter
    }
}

pub trait Tally {
    fn tally();
}

impl Tally for Counter {
    fn tally() {}
}

static START: u32 = start_value(); // no source: outside any function

pub const fn start_value() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check() {
        super::local_fn(); // path-exact
        local_fn(); // none: only the glob `super::*` supplies it
    }
}
