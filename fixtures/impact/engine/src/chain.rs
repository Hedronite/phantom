//! A chain to depth 4, and fan-in on `root_fn`.

use crate::root_fn;

pub fn one() -> u32 {
    root_fn()
}

pub fn two() -> u32 {
    one()
}

pub fn three() -> u32 {
    two()
}

pub fn four() -> u32 {
    three()
}

pub fn fan_a() -> u32 {
    crate::root_fn()
}

pub fn fan_b() -> u32 {
    super::root_fn() + one()
}
