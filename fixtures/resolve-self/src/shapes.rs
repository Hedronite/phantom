//! The same method names on two types, and an impl reached by path.

pub enum Shape {
    Circle(f64),
    Square(f64),
}

impl Shape {
    pub fn unit() -> Self {
        Self::Circle(1.0)
    }

    pub fn area(&self) -> f64 {
        0.0
    }

    pub fn describe(&self) {
        self.area();
    }
}

pub struct Plot;

impl Plot {
    pub fn area(&self) -> f64 {
        1.0
    }

    pub fn describe(&self) {
        self.area();
        Self::area(self);
    }
}

impl super::Counter {
    pub fn from_shapes(&self) {
        self.check();
    }
}
