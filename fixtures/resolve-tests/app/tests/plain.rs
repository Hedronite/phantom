//! An integration test file: a `test` target of its own.

mod common;

use app::Counter;
use app::add as plus;
use common::fresh as fresh_counter;

#[test]
fn own_lib_by_crate_path() {
    let _ = app::add(1, 2);
}

#[test]
fn dev_dependency_by_crate_path() {
    let _ = probe::check(1);
}

#[test]
fn through_a_use_alias() {
    let _ = fresh_counter();
    let _ = plus(2, 2);
}

#[test]
fn into_common() {
    let _ = common::fresh();
}

#[test]
fn a_typed_method_call() {
    let mut counter = Counter::new();
    let _ = counter.bump();
}
