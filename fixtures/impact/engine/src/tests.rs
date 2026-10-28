//! A `#[cfg(test)] mod tests;` file module.

fn helper() -> u32 {
    crate::root_fn()
}

#[test]
fn t_root() {
    let v = super::root_fn();
    assert_eq!(v + helper(), 2);
}
