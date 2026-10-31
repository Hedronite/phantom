//! Typed receivers for the call resolver (SPEC-w10 G2a). Never built or run.
//! Each function below is one case in `EXPECTED.md`.

use std::rc::Rc;
use std::sync::Arc;

use geo::Point;

pub mod clears;

pub struct Counter {
    n: u32,
}

pub struct Gauge;

pub struct Wrap<T> {
    inner: T,
}

pub struct Error;

pub trait Reader {
    fn peek(&self) -> u32;
}

pub trait Left {
    fn go(&self);
}

pub trait Right {
    fn go(&self);
}

impl Counter {
    pub fn new() -> Self {
        Counter { n: 0 }
    }

    pub fn fresh() -> Counter {
        Counter { n: 0 }
    }

    pub fn open() -> Result<Self, Error> {
        Ok(Counter { n: 0 })
    }

    pub fn find() -> Option<Counter> {
        None
    }

    pub fn count() -> u32 {
        0
    }

    pub fn read(&self) -> u32 {
        self.n
    }

    pub fn bump(&mut self) {
        self.n += 1;
    }

    pub fn twin(&self) -> u32 {
        let other = Self::new();
        other.read()
    }

    pub fn merge(&self, other: Self) -> u32 {
        other.read()
    }

    pub fn literal(&self) -> u32 {
        let made = Self { n: 1 };
        made.read()
    }
}

impl Gauge {
    pub fn new() -> Gauge {
        Gauge
    }

    pub fn read(&self) -> u32 {
        1
    }
}

impl Reader for Gauge {
    fn peek(&self) -> u32 {
        2
    }
}

impl Left for Counter {
    fn go(&self) {}
}

impl Right for Counter {
    fn go(&self) {}
}

impl<T> Wrap<T> {
    pub fn get(&self) -> &T {
        &self.inner
    }
}

pub fn make() -> Counter {
    Counter::new()
}

pub fn by_param(c: Counter) -> u32 {
    c.read()
}

pub fn by_ref(c: &Counter) -> u32 {
    c.read()
}

pub fn by_mut(c: &mut Counter) {
    c.bump();
}

pub fn by_box(c: Box<Counter>) -> u32 {
    c.read()
}

pub fn by_rc(c: Rc<Counter>) -> u32 {
    c.read()
}

pub fn by_arc(c: Arc<Counter>) -> u32 {
    c.read()
}

pub fn by_scoped(c: crate::Counter) -> u32 {
    c.read()
}

pub fn by_generic(w: Wrap<u8>) {
    w.get();
}

pub fn let_typed() -> u32 {
    let c: Counter = make();
    c.read()
}

pub fn let_wrapped() -> u32 {
    let c: &mut Box<Gauge> = todo();
    let g: Arc<Gauge> = todo();
    g.read() + c.read()
}

pub fn let_literal() -> u32 {
    let c = Counter { n: 2 };
    c.read()
}

pub fn ctor_self() -> u32 {
    let c = Counter::new();
    c.read()
}

pub fn ctor_named() -> u32 {
    let c = Counter::fresh();
    c.read()
}

pub fn ctor_result() -> Result<u32, Error> {
    let c = Counter::open()?;
    Ok(c.read())
}

pub fn ctor_option() -> Option<u32> {
    let c = Counter::find()?;
    Some(c.read())
}

pub fn ctor_not_self() -> u32 {
    let n = Counter::count();
    n.read()
}

pub fn ctor_untried() -> u32 {
    let c = Counter::open();
    c.read()
}

pub fn far(p: Point) -> u32 {
    p.norm()
}

pub fn far_ctor() -> u32 {
    let p = geo::Point::new();
    p.norm()
}

pub fn shadowed(c: Counter) -> u32 {
    let c = Gauge::new();
    c.read()
}

pub fn shadow_ends(c: Counter) -> u32 {
    {
        let c = Gauge::new();
        c.read();
    }
    c.read()
}

pub fn in_closure(c: Counter) -> u32 {
    let f = || c.read();
    f()
}

pub fn two_traits(c: Counter) {
    c.go();
}

pub fn trait_impl_method(r: Gauge) -> u32 {
    r.peek()
}
