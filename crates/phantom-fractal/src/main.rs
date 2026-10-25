//! Phantom command line. Builds as both `phantom-fractal` and `fract`.
//!
//! Draft slice: version and help text only. No subcommands yet.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

/// Canon exit 1: usage or internal error.
const EXIT_ERROR: u8 = 1;

/// Both binary names print the same text, so the name is fixed here
/// rather than taken from argv[0].
#[derive(Parser)]
#[command(
    name = "phantom-fractal",
    bin_name = "phantom-fractal",
    version,
    help_template = "{name} {version}"
)]
struct Cli {}

fn main() -> ExitCode {
    match Cli::try_parse() {
        Ok(Cli {}) => match Cli::command().print_help() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(EXIT_ERROR),
        },
        Err(err) => {
            let code = if err.use_stderr() {
                ExitCode::from(EXIT_ERROR)
            } else {
                ExitCode::SUCCESS
            };
            // If printing fails there is no stream left to report it on.
            let _ = err.print();
            code
        }
    }
}
