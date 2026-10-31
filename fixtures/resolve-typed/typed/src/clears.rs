//! Bindings that clear a receiver's hint (SPEC-w10 G2a). Never built or
//! run. Each parameter `c: Counter` would type `c`; the nearer binding
//! clears it, so no call here gets a `typed-method` edge.

use crate::{Counter, Reader, make};

pub fn untyped_let(c: Counter) -> u32 {
    let c = make();
    c.read()
}

pub fn inferred_let(c: Counter) -> u32 {
    let c: _ = make();
    c.read()
}

pub fn in_match(c: Counter, o: Option<Counter>) -> u32 {
    match o {
        Some(c) => c.read(),
        None => 0,
    }
}

pub fn in_if_let(c: Counter, o: Option<Counter>) -> u32 {
    if let Some(c) = o { c.read() } else { 0 }
}

pub fn in_while_let(c: Counter, mut o: Vec<Counter>) {
    while let Some(c) = o.pop() {
        c.read();
    }
}

pub fn in_for(c: Counter, all: Vec<Counter>) {
    for c in all {
        c.read();
    }
}

pub fn closure_parameter(c: Counter) {
    let f = |c: Counter| c.read();
}

pub fn tuple_pattern(c: Counter, pair: (Counter, u8)) -> u32 {
    let (c, _) = pair;
    c.read()
}

pub fn struct_pattern(c: Counter, h: Holder) -> u32 {
    let Holder { c } = h;
    c.read()
}

pub fn impl_trait(c: impl Reader) -> u32 {
    c.peek()
}

pub fn dyn_trait(c: &dyn Reader) -> u32 {
    c.peek()
}

pub fn untyped_closure(c: Counter) {
    let f = |c| c.read();
}

pub struct Holder {
    c: Counter,
}
