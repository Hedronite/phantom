//! The dogfood cures of SPEC-w13 G2a (D9).

mod alt;
mod cycle;
mod mid;
mod model;
pub mod prelude;

use alt::*;
use model::*;

pub fn local_wins() -> u32 {
    0
}

pub fn entry() -> u32 {
    let one = build(); // (b) one candidate: only `model` declares `build`
    twin(); // (b) two candidates: `model` and `alt` both declare `twin`
    local_wins(); // (b) shadowed: a local item beats the glob
    model::build(); // already placed by the base tiers: keeps `local`
    prelude::Gadget::new(); // (c) two hops: prelude, then mid, then model
    cycle::looped(); // (c) a cycle of re-exports: refused
    one
}
