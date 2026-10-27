//! A local `mod core_lib` wins over the crate of that name.

mod core_lib {
    pub fn f() {}
}

pub fn go() {
    core_lib::f();
}
