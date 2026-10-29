//! The lib `app` imports from and implements.

pub mod geo;

/// Something with an area.
pub trait Shape {
    fn area(&self) -> f64;
    fn sides(&self) -> u32;
}

pub fn scale(x: f64) -> f64 {
    x * 2.0
}
