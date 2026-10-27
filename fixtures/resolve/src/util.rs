//! A non-`mod.rs` parent: `mod deep;` is `util/deep.rs`.

mod deep;

pub fn helper() {
    deep::sink(); // local
    super::local_fn(); // path-exact
}
