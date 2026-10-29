pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub trait Describe {
    fn describe(&self) -> String;
}
