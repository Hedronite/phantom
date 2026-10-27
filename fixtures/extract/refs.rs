//! Call reference fixture for the Phantom extract pack (SPEC-w3 G0a).
//! Written for this pack. Not compiled.

pub fn plain(input: &str) -> u32 {
    helper();
    crate::helpers::helper();
    Ledger::new();
    Vec::<u8>::with_capacity(4);
    let parsed = input.trim().parse::<u32>();
    println!("{}", hidden_in_macro());
    std::mem::drop(parsed);
    0
}

pub struct Ledger;

impl Ledger {
    pub fn new() -> Self {
        let build = |n: u32| helper_in_closure(n);
        build(1);
        Self
    }
}

static TABLE: [u8; 1] = [first_byte()];

mod tests {
    fn checks() {
        super::plain("x");
    }
}

pub trait Greet {
    fn greet(&self) {
        say_hello();
    }
}
