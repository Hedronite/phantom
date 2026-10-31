//! `version` and the global `--json`: one envelope line, the same under
//! every spelling and both binaries, with no index, no store, no data
//! directory, and no `HOME` (SPEC-w5 G3c).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

/// Every spelling Lapis may use, with each binary.
const SPELLINGS: [&[&str]; 3] = [&["version"], &["--json", "version"], &["version", "--json"]];

/// A fresh, empty directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-fractal-version-{}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path.canonicalize().expect("canonical temp dir"))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn is_empty(&self) -> bool {
        fs::read_dir(&self.0)
            .expect("read temp dir")
            .next()
            .is_none()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `bin ARGS` in an empty directory with `HOME` unset and the data
/// directory pointed at a path that does not exist. Neither may appear.
fn run_bare(bin: &str, args: &[&str]) -> Output {
    let dir = TempDir::new();
    let data = dir.path().join("data");
    let out = Command::new(bin)
        .args(args)
        .env_remove("HOME")
        .env_remove("XDG_DATA_HOME")
        .env("PHANTOM_FRACTAL_DATA", &data)
        .current_dir(dir.path())
        .output()
        .unwrap_or_else(|err| panic!("cannot run {bin}: {err}"));
    assert!(dir.is_empty(), "{bin} {args:?} wrote into its directory");
    out
}

/// 40 lowercase hex characters.
fn is_full_sha(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The commit this test was built from, when git can say: the override
/// `build.rs` honours, else `git rev-parse HEAD` beside this crate.
fn built_sha() -> Option<String> {
    if let Some(sha) = option_env!("PHANTOM_FRACTAL_GIT_SHA") {
        let sha = sha.trim().to_ascii_lowercase();
        if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(sha);
        }
    }
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && is_full_sha(&sha)).then_some(sha)
}

#[test]
fn every_spelling_prints_the_same_envelope_line() {
    let first = run_bare(BINS[0], SPELLINGS[0]);
    let line = String::from_utf8(first.stdout).expect("utf-8");
    for bin in BINS {
        for args in SPELLINGS {
            let out = run_bare(bin, args);
            assert_eq!(out.status.code(), Some(0), "{bin} {args:?}");
            assert_eq!(String::from_utf8_lossy(&out.stdout), line, "{bin} {args:?}");
            assert!(out.stderr.is_empty(), "{bin} {args:?} wrote stderr");
        }
    }
}

#[test]
fn the_line_is_the_spec_envelope() {
    let out = run_bare(BINS[1], &["version"]);
    let line = String::from_utf8(out.stdout).expect("utf-8");
    let env: Value = serde_json::from_str(line.trim_end()).expect("one JSON envelope");
    let sha = &env["data"]["git_sha"];
    let sha_text = match sha {
        Value::Null => "null".to_owned(),
        Value::String(sha) => {
            assert!(is_full_sha(sha), "git_sha {sha:?}");
            format!("\"{sha}\"")
        }
        other => panic!("git_sha is {other}"),
    };
    // Keys in this order: ok, data, error, schema; and in data, name,
    // version, git_sha, extractor_version, store_schema.
    let want = format!(
        "{{\"ok\":true,\"data\":{{\"name\":\"phantom-fractal\",\"version\":\"0.0.0\",\
         \"git_sha\":{sha_text},\"extractor_version\":{},\"store_schema\":{}}},\
         \"error\":null,\"schema\":{}}}\n",
        phantom_fractal_extract::EXTRACTOR_VERSION,
        phantom_fractal_store::SCHEMA_VERSION,
        phantom_fractal_proto::SCHEMA_VERSION,
    );
    assert_eq!(line, want);
    assert_eq!(env["data"]["extractor_version"], 9, "{env}");
    assert_eq!(env["data"]["store_schema"], 3, "{env}");
    assert_eq!(env["schema"], 1, "{env}");
}

#[test]
fn git_sha_is_the_built_commit() {
    let out = run_bare(BINS[0], &["version"]);
    let env: Value = serde_json::from_slice(&out.stdout).expect("one JSON envelope");
    let got = env["data"]["git_sha"].as_str().map(str::to_owned);
    match built_sha() {
        Some(want) => assert_eq!(got.as_deref(), Some(want.as_str()), "{env}"),
        None => assert!(got.as_deref().is_none_or(is_full_sha), "{env}"),
    }
}

/// An extra argument is usage, exit 1, with no envelope.
#[test]
fn version_takes_no_argument() {
    let cases: [&[&str]; 4] = [
        &["version", "extra"],
        &["version", "--repo", "."],
        &["version", "--depth", "1"],
        &["--json", "version", "x"],
    ];
    for bin in BINS {
        for args in cases {
            let out = run_bare(bin, args);
            assert_eq!(out.status.code(), Some(1), "{bin} {args:?}");
            assert!(out.stdout.is_empty(), "{bin} {args:?} wrote stdout");
            assert!(!out.stderr.is_empty(), "{bin} {args:?} wrote no error");
        }
    }
}

/// The plain version line is not the envelope, under `--json` too.
#[test]
fn the_version_flag_keeps_its_plain_line() {
    for bin in BINS {
        for args in [&["--version"][..], &["-V"], &["--json", "--version"]] {
            let out = run_bare(bin, args);
            assert_eq!(out.status.code(), Some(0), "{bin} {args:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "phantom-fractal 0.0.0\n",
                "{bin} {args:?}"
            );
        }
    }
}
