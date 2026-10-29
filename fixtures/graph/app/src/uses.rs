use crate::dup::Dup;
use crate::shapes;
use crate::shapes::Circle;
use crate::shapes::Square as Block;
use crate::shapes::{Tile, Wrapper};
use kit::geo::Point;
use kit::scale as grow;
use shapes::Circle as Round;
use std::collections::HashMap;
use super::missing::Gone;

mod inner {
    use super::Circle;

    pub fn deep() -> Circle {
        Circle::new(1.0)
    }
}

use inner::deep;

pub fn all() -> usize {
    let _ = (Dup(), Block { side: grow(1.0) }, Tile::Wall, Wrapper(1_u8));
    let _ = (Point { x: 0.0, y: 0.0 }, Round::new(2.0), deep(), shapes::Circle::new(3.0));
    let map: HashMap<u8, Gone> = HashMap::new();
    map.len()
}
