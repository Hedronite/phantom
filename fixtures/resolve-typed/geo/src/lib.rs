//! A type in another workspace crate (SPEC-w10 G2a). Never built or run.

pub struct Point;

impl Point {
    pub fn new() -> Self {
        Point
    }

    pub fn norm(&self) -> u32 {
        0
    }
}
