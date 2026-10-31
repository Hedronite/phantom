//! A directory test target. Its `common` is the file `plain` already
//! holds, so it gets no module here and the call into it stays unresolved.

#[path = "../common/mod.rs"]
mod common;

#[test]
fn suite_case() {
    let _ = app::add(3, 4);
    let _ = common::fresh();
}
