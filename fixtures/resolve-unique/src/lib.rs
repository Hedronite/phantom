//! The `unique-method` tier (SPEC-w10 G3a). Never built or run. Each
//! function below is one case in `EXPECTED.md`.

pub struct Ledger;

pub struct Book;

pub struct Shelf {
    ledger: Ledger,
}

impl Ledger {
    pub fn settle(&self) {}

    pub fn len(&self) -> usize {
        0
    }

    pub fn label(&self) {}
}

impl Book {
    pub fn label(&self) {}

    pub fn shelve(&self) {
        self.settle();
    }
}

pub fn make() -> Ledger {
    Ledger
}

pub fn one_unique_name() {
    make().settle();
}

pub fn a_field_receiver(s: &Shelf) {
    s.ledger.settle();
}

pub fn a_cleared_hint() {
    let f = |x| x.settle();
}

pub fn a_stoplisted_name() -> usize {
    make().len()
}

pub fn a_name_defined_twice() {
    make().label();
}

pub fn a_hint_that_places_nothing(v: Vec<u8>) {
    v.settle();
}

pub fn a_typed_call_stays_typed(l: Ledger) {
    l.settle();
}
