pub fn aux() {}

/// Nearby entries filtered by root predicate.
pub fn nearby_entries(flag: bool) -> bool {
    /// Returns whether `path` is root.
    fn is_root(path: &str) -> bool {
        aux();
        path == "/"
    }
    if flag {
        is_root("/")
    } else {
        false
    }
}

pub struct App;

impl App {
    pub fn run(&self) {
        fn step() {
            aux();
        }
        step();
    }
}
