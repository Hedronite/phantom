//! Shared by both test roots. `plain` reaches it first, so it is a module
//! of `plain` only.

use app::Counter;

pub fn fresh() -> Counter {
    start()
}

fn start() -> Counter {
    Counter::new()
}
