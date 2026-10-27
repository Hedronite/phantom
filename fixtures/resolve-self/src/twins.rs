//! `#[cfg]` twin impls give two candidates, so no edge.

pub struct Gate;

#[cfg(unix)]
impl Gate {
    fn m(&self) {}
}

#[cfg(not(unix))]
impl Gate {
    fn m(&self) {}
}

impl Gate {
    pub fn open(&self) {
        self.m();
        Self::m(self);
    }
}
