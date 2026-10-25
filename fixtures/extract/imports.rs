//! Module and use fixture for the Phantom extract pack (SPEC-extract G2b).
//! Written for this pack. Not compiled.

mod parser;
pub mod render;
pub(crate) mod cache {
    use super::render::Frame;
}

use std::collections::HashMap;
use std::io::Result as IoResult;
pub use crate::render::Canvas;
use std::fmt::*;
pub(crate) use self::cache::{
    self as store,
    Entry,
    nested::*,
};
use ::core::mem;
use std::io::Write as _;

fn scoped() {
    use std::cell::Cell;
}
