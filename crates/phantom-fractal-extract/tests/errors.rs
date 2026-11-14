//! How `index`, `probe` and `diff` fail, and how their errors read.
//!
//! An unreadable source file fails an index as I/O. A repository whose
//! HEAD cannot be read fails a probe as git. A missing root fails a diff
//! as I/O, and an unknown base fails it with the git error's own text.

mod common;

use std::error::Error as _;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{TempDir, write};
use phantom_fractal_extract::{DiffError, IndexError, diff, index, probe};
use phantom_fractal_git::{Base, ChangesError};
use phantom_fractal_lang::ExtractError;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{Store, StoreError};

const MANIFEST: &[u8] = b"[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

fn store(dir: &TempDir) -> Store {
    Store::open(&dir.path().join("graph.db")).expect("open store")
}

fn rust() -> RustLanguage {
    RustLanguage::new().expect("the query compiles")
}

/// Sets the mode of `path`.
fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn an_unreadable_source_file_fails_the_index_as_io() {
    let root = TempDir::new("errors-unreadable");
    write(root.path(), "Cargo.toml", MANIFEST);
    write(root.path(), "src/lib.rs", b"pub fn f() {}\n");
    let locked = root.path().join("src/lib.rs");
    chmod(&locked, 0o000);
    if fs::read(&locked).is_ok() {
        // A superuser reads it anyway; there is nothing to fail.
        chmod(&locked, 0o644);
        return;
    }
    let db = TempDir::new("errors-unreadable-db");
    let got = index(root.path(), &mut store(&db), &rust());
    chmod(&locked, 0o644);
    let Err(err @ IndexError::Io(_)) = got else {
        panic!("{got:?}");
    };
    assert!(err.to_string().starts_with("walk failed: "), "{err}");
    assert!(err.source().is_some());
}

/// Runs `git` in `dir` with no system or global config.
fn git(dir: &Path, home: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn a_repository_whose_head_cannot_be_read_fails_the_probe_as_git() {
    let root = TempDir::new("errors-head");
    let home = TempDir::new("errors-head-home");
    write(root.path(), "Cargo.toml", MANIFEST);
    write(root.path(), "src/lib.rs", b"pub fn f() {}\n");
    git(root.path(), home.path(), &["init", "-q", "-b", "main"]);
    // HEAD names a branch whose ref holds no object id.
    write(root.path(), ".git/refs/heads/main", b"not an object id\n");
    let db = TempDir::new("errors-head-db");
    let got = probe(root.path(), &store(&db));
    let Err(err @ IndexError::Git(_)) = got else {
        panic!("{got:?}");
    };
    let IndexError::Git(inner) = &err else {
        unreachable!();
    };
    assert_eq!(err.to_string(), inner.to_string());
    assert!(err.source().is_some());
}

#[test]
fn a_store_error_reads_as_store_failed() {
    let err = IndexError::from(StoreError::MissingProducer);
    assert!(matches!(
        err,
        IndexError::Store(StoreError::MissingProducer)
    ));
    assert_eq!(
        err.to_string(),
        format!("store failed: {}", StoreError::MissingProducer)
    );
    assert!(err.source().is_some());
}

#[test]
fn a_diff_of_a_missing_root_fails_as_io() {
    let dir = TempDir::new("errors-diff-missing");
    let got = diff(&dir.path().join("gone"), &Base::Head, &rust());
    let Err(err @ DiffError::Io(_)) = got else {
        panic!("{got:?}");
    };
    assert!(err.to_string().starts_with("walk failed: "), "{err}");
}

#[test]
fn an_unknown_base_reads_as_the_git_error() {
    let err = DiffError::from(ChangesError::UnknownBase);
    assert_eq!(err.to_string(), ChangesError::UnknownBase.to_string());
}

#[test]
fn an_extractor_refusal_reads_as_extract() {
    let err = DiffError::from(ExtractError::Parse);
    assert!(matches!(err, DiffError::Extract(ExtractError::Parse)));
    assert_eq!(err.to_string(), format!("extract: {}", ExtractError::Parse));
}
