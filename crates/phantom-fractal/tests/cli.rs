//! Both binaries print one version line, list all ten commands in help,
//! and refuse every other command.

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
fn version_prints_the_version_line() {
    let cases: [&[&str]; 2] = [&["--version"], &["-V"]];
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

/// `--help` and `-h` exit 0. A bare run is usage, so it exits 1, not clap's 2.
#[test]
fn help_lists_every_command() {
    let cases: [(&[&str], i32); 3] = [(&["--help"], 0), (&["-h"], 0), (&[], 1)];
    for bin in BINS {
        for (args, code) in cases {
            let out = run(bin, args);
            assert_eq!(out.status.code(), Some(code), "{bin} {args:?}");
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            for command in [
                "index", "find", "callers", "callees", "impact", "show", "rank", "map", "version",
                "mcp",
            ] {
                assert!(text.contains(command), "{bin} {args:?}\n{text}");
            }
            assert!(text.contains("Usage"), "{bin} {args:?}\n{text}");
        }
    }
}

#[test]
fn other_commands_and_unknown_flags_exit_one() {
    let cases: [&[&str]; 11] = [
        &["serve"],
        &["impact"],
        &["watch", "--interval-ms", "1"],
        &["help"],
        &["--json"],
        &["rank", "open_ledger"],
        &["index"],
        &["find"],
        &["callers"],
        &["callees"],
        &["callers", "a", "b"],
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
