use crate::traits::Named;
use kit::Shape;

pub struct Circle {
    pub r: f64,
}

pub struct Square {
    pub side: f64,
}

pub enum Tile {
    Floor,
    Wall,
}

pub struct Wrapper<T>(pub T);

impl Circle {
    pub fn new(r: f64) -> Self {
        Circle { r }
    }
}

impl Named for Circle {
    fn name(&self) -> &'static str {
        "circle"
    }
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        3.14 * self.r * self.r
    }

    fn sides(&self) -> u32 {
        0
    }
}

impl crate::traits::Named for Tile {
    fn name(&self) -> &'static str {
        "tile"
    }
}

impl kit::Shape for Tile {
    fn area(&self) -> f64 {
        1.0
    }

    fn sides(&self) -> u32 {
        4
    }
}

impl kit::geo::Describe for Square {
    fn describe(&self) -> String {
        String::from("square")
    }
}

impl Missing for Square {
    fn gone(&self) {}
}

impl std::fmt::Display for Square {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.side)
    }
}

impl Clone for Tile {
    fn clone(&self) -> Self {
        Tile::Floor
    }
}

impl Named for Wrapper<u8> {
    fn name(&self) -> &'static str {
        "byte"
    }
}

impl Named for Wrapper<u16> {
    fn name(&self) -> &'static str {
        "short"
    }
}
