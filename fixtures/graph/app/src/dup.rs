//! `Dup` names a struct in the type namespace and a function in the value
//! namespace, so a `use` of it names two symbols. The glob sits here, not
//! in `uses`, because a glob could supply any first segment of its module.

use crate::traits::*;

pub struct Dup {}

#[allow(non_snake_case)]
pub fn Dup() -> Dup {
    Dup {}
}

impl Named for Dup {
    fn name(&self) -> &'static str {
        "dup"
    }
}
