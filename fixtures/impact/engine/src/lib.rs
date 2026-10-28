//! Impact fixture, the lib (SPEC-w5 G2d). Written for this pack. Not compiled.

mod chain;
mod ring;
pub mod shapes;
#[cfg(test)]
mod tests;

pub fn root_fn() -> u32 {
    1
}

fn hidden() -> u32 {
    2
}

pub fn uses_hidden() -> u32 {
    hidden()
}

#[test]
fn direct_test() {
    let _ = hidden();
}

#[cfg(test)]
mod inline_tests {
    fn helper() -> u32 {
        super::hidden()
    }

    fn outer() -> u32 {
        helper()
    }
}

pub fn used_in_macros() -> u32 {
    3
}

pub fn real() -> u32 {
    used_in_macros()
}

pub fn checks() {
    assert!(used_in_macros() > 0);
    let v = vec![used_in_macros()];
    drop(v);
}
