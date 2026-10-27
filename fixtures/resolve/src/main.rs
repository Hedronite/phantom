//! The package's bin target.

mod cli;

fn main() {
    cli::run(); // local
    resolve_fixture::entry(); // external: another crate by name
}
