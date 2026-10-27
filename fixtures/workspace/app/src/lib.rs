//! A lib with a path dependency on `core-lib`.

use core_lib::Thing as CoreThing;
use core_lib::util::helper;

pub fn run() {
    core_lib::f();
    core_lib::util::helper();
    core_lib::again();
    helper();
    CoreThing::new();
    core_lib::Thing::new();
    core_lib::missing();
    app::run();
}

pub trait Show {
    fn show(&self);

    fn twice(&self);
}

impl Show for core_lib::Thing {
    fn show(&self) {}

    fn twice(&self) {
        self.show();
    }
}
