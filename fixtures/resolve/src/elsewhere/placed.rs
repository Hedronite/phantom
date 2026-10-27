//! Reached only through `#[path = "elsewhere/placed.rs"]` in `lib.rs`.

pub fn from_path() {
    super::local_fn(); // path-exact: the parent is the crate root
}
