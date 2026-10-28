//! Embeds the commit the binary is built from, for `version`.
//!
//! `PHANTOM_FRACTAL_GIT_SHA`, when it is 40 hex characters, wins; it is
//! for packagers. Otherwise `git rev-parse HEAD` in this crate's directory
//! gives it, when the output is 40 hex characters. Anything else (no git,
//! no `.git` as in a crates.io tarball, a dubious-ownership refusal) embeds
//! nothing, and `version` prints `null`. This script never fails the build
//! and prints nothing but cargo directives.

#![forbid(unsafe_code)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const OVERRIDE: &str = "PHANTOM_FRACTAL_GIT_SHA";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed={OVERRIDE}");
    let dir = env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let sha = env::var(OVERRIDE)
        .ok()
        .and_then(|value| full_sha(&value))
        .or_else(|| {
            watch_head(&dir);
            git(&dir, &["rev-parse", "HEAD"]).and_then(|out| full_sha(&out))
        })
        .unwrap_or_default();
    println!("cargo:rustc-env=PHANTOM_FRACTAL_BUILD_GIT_SHA={sha}");
}

/// `text`, trimmed and lowercased, when it is exactly 40 hex characters.
fn full_sha(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// Reruns this script when `HEAD` or the branch it points at moves, so a
/// new commit is embedded. Paths come from `git rev-parse --git-path`, so
/// a worktree's `HEAD` and the shared refs are both found. `packed-refs`
/// is watched only when it exists, since cargo reruns a script on every
/// build while a watched path is missing.
fn watch_head(dir: &Path) {
    let mut watched = vec![("HEAD".to_owned(), true), ("packed-refs".to_owned(), false)];
    if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        watched.push((branch, true));
    }
    for (name, always) in watched {
        if let Some(path) = git(dir, &["rev-parse", "--git-path", &name]) {
            let path = dir.join(path);
            if always || path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

/// The trimmed stdout of `git ARGS` run in `dir`, when it succeeds and is
/// UTF-8. Its stderr is captured and dropped.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
