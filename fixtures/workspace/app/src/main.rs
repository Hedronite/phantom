//! The bin of `app`, calling its own lib and `core-lib` by crate name.

fn main() {
    app::run();
    core_lib::f();
}
