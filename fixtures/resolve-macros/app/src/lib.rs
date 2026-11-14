mod helpers;
mod shapes;

use helpers::double;
use shapes::{Book, Ledger};

/// A user macro. Its body is never scanned.
macro_rules! check {
    ($e:expr) => {
        hidden_in_body($e)
    };
}

pub fn local_one() -> u32 {
    1
}

fn hidden_in_body(x: u32) -> u32 {
    x
}

fn make() -> Ledger {
    Ledger::open()
}

pub fn local_in_assert() {
    assert!(local_one() > 0);
}

pub fn use_alias_in_assert_eq() {
    assert_eq!(double(2), 4);
}

pub fn crate_path_in_vec() -> Vec<u32> {
    vec![util::triple(1)]
}

pub fn path_exact_in_format() -> String {
    format!("{}", crate::helpers::double(3))
}

pub fn assoc_in_a_user_macro() -> u32 {
    check!(Ledger::count())
}

pub fn a_hinted_receiver(ledger: &Ledger) {
    assert_eq!(ledger.total(), 0);
}

pub fn a_constructed_receiver() {
    let book = Book::new();
    assert_eq!(book.total(), 0);
}

pub fn an_unhinted_receiver() {
    assert_eq!(make().settle(), ());
}

pub fn a_stoplisted_name() {
    assert_eq!(make().len(), 0);
}

pub fn a_shadowed_name() {
    let local_one = || 2;
    assert!(local_one() > 1);
}

pub fn an_external_path() {
    assert!(std::mem::size_of::<u32>() == 4);
}
