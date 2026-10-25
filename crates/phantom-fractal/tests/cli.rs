//! Both binaries print one version line and refuse every command but
//! `index` and `find`.

use std::process::{Command, Output};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

const VERSION_LINE: &str = "phantom-fractal 0.0.0\n";

fn run(bin: &str, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("cannot run {bin}: {err}"))
}

#[test]
fn version_help_and_bare_print_the_version_line() {
    let cases: [&[&str]; 5] = [&["--version"], &["-V"], &["--help"], &["-h"], &[]];
    for bin in BINS {
        for args in cases {
            let out = run(bin, args);
            assert_eq!(out.status.code(), Some(0), "{bin} {args:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                VERSION_LINE,
                "{bin} {args:?}"
            );
            assert!(out.stderr.is_empty(), "{bin} {args:?} wrote stderr");
        }
    }
}

#[test]
fn other_commands_and_unknown_flags_exit_one() {
    let cases: [&[&str]; 9] = [
        &["mcp"],
        &["serve"],
        &["impact"],
        &["map"],
        &["help"],
        &["--json"],
        &["callers", "open_ledger"],
        &["index"],
        &["find"],
    ];
    for bin in BINS {
        for args in cases {
            let out = run(bin, args);
            assert_eq!(out.status.code(), Some(1), "{bin} {args:?}");
            assert!(out.stdout.is_empty(), "{bin} {args:?} wrote stdout");
            assert!(!out.stderr.is_empty(), "{bin} {args:?} wrote no error");
        }
    }
}
