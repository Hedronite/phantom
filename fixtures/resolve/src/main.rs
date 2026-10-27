//! The package's bin target.

mod cli;

fn main() {
    cli::run(); // local
    outside_crate::entry(); // external: a crate outside the workspace
}
