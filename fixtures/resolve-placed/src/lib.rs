mod other;

use other::Far;

pub struct Ledger {
    total: u32,
}

pub struct Book;

pub struct Shelf {
    ledger: Box<Ledger>,
}

pub struct Wrap<T> {
    inner: T,
}

impl Ledger {
    pub fn open() -> Self {
        Ledger { total: 0 }
    }

    pub fn shelve() -> Book {
        Book
    }

    pub fn label(&self) -> u32 {
        self.total
    }
}

impl Book {
    pub fn label(&self) -> u32 {
        0
    }

    pub fn again(&self) -> Book {
        Book
    }

    pub fn settle(&self) {}
}

fn make_book() -> Book {
    Book
}

fn find_book() -> Option<Book> {
    Some(Book)
}

pub fn ctor_other() -> u32 {
    let b = Ledger::shelve();
    b.label()
}

pub fn returned() -> u32 {
    let b = make_book();
    b.label()
}

pub fn returned_tried() -> Option<u32> {
    let b = find_book()?;
    Some(b.label())
}

pub fn call_receivers() -> u32 {
    make_book().label() + Ledger::shelve().label()
}

impl Shelf {
    pub fn same_file_field(&self) -> u32 {
        self.ledger.label()
    }
}

impl<T> Wrap<T> {
    pub fn generic_field(&self) -> u32 {
        self.inner.label()
    }
}

impl Far {
    pub fn other_file_field(&self) -> u32 {
        self.book.label()
    }
}

pub fn method_chain() -> u32 {
    let b = make_book();
    b.again().label()
}

pub fn keeps_unique() {
    make_book().settle();
}

pub fn shadowed_fn() -> u32 {
    let make_book = || Ledger::open();
    let b = make_book();
    b.label()
}
