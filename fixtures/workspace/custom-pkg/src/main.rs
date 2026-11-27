//! The bin of `custom-pkg`, calling its own lib by custom `[lib] name`.

fn main() {
    custom::api();
    custom_pkg::api();
}
