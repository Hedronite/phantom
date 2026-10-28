//! A trait-impl method target, reached through a `self` call.

pub trait Area {
    fn area(&self) -> u32;
}

pub struct Square(pub u32);

impl Area for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

impl Square {
    pub fn new(side: u32) -> Self {
        Self(side)
    }

    pub fn doubled(&self) -> u32 {
        self.area() * 2
    }
}
