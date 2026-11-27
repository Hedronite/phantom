//! An integration test of `custom-pkg`, calling its own lib by custom `[lib] name`.

#[test]
fn check() {
    custom::api();
}
