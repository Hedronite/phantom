//! A `mod.rs` parent: `mod circle;` is `shapes/circle.rs`.

mod circle;

pub fn area() -> u32 {
    circle::radius() // local
}
