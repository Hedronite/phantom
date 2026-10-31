//! The bin calls its own lib, and not the dev-dependency.

fn main() {
    let total = app::add(1, 2);
    let _ = probe::check(total);
}
