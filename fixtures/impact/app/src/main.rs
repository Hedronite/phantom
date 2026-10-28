//! Impact fixture, the bin (SPEC-w5 G2d). Written for this pack. Not compiled.

use engine::shapes::Area;

fn main() {
    run();
}

fn run() -> u32 {
    let square = engine::shapes::Square::new(3);
    let twice = engine::shapes::Square::doubled(&square);
    let other = square.area();
    twice + other + engine::root_fn() + report()
}

fn report() -> u32 {
    engine::root_fn()
}
