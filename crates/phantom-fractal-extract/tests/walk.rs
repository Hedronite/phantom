//! The walk over the index fixture (SPEC-index G1a).

mod common;

use std::collections::BTreeMap;

use common::FixtureRepo;
use phantom_fractal_extract::{SkipReason, is_secret, walk};

#[test]
fn walk_keeps_only_plain_files_inside_the_rules() {
    let repo = FixtureRepo::new();
    let got = walk(repo.root()).expect("walk");
    let mut kept: Vec<&str> = got.files.iter().map(|file| file.path.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        [
            ".gitignore",
            "Cargo.toml",
            "README.txt",
            "edge.rs",
            "src/ledger.rs",
            "src/lib.rs",
            "src/notes.txt",
            "src/target/mod.rs",
        ]
    );
}

#[test]
fn walk_records_why_each_seen_file_was_skipped() {
    use SkipReason::{Secret, SymlinkInRoot, SymlinkLeavesRoot, TooLarge};
    let repo = FixtureRepo::new();
    let got = walk(repo.root()).expect("walk");
    let skipped: BTreeMap<&str, SkipReason> = got
        .skipped
        .iter()
        .map(|skip| (skip.path.as_str(), skip.reason))
        .collect();
    let expected = BTreeMap::from([
        (".env", Secret),
        (".env.local", Secret),
        ("big.rs", TooLarge),
        ("certs/deploy.key", Secret),
        ("certs/server.pem", Secret),
        ("dangling.rs", SymlinkLeavesRoot),
        ("keys/id_ed25519", Secret),
        ("link_in.rs", SymlinkInRoot),
        ("link_out.rs", SymlinkLeavesRoot),
        ("linked_dir", SymlinkLeavesRoot),
        ("src/id_codec.rs", Secret),
    ]);
    assert_eq!(skipped, expected);
}

#[test]
fn walk_prunes_git_target_and_ignored_paths_without_a_record() {
    let repo = FixtureRepo::new();
    let got = walk(repo.root()).expect("walk");
    let seen = got
        .files
        .iter()
        .map(|file| file.path.as_str())
        .chain(got.skipped.iter().map(|skip| skip.path.as_str()));
    for path in seen {
        assert!(!path.starts_with(".git/"), "{path}");
        assert!(!path.starts_with("target/"), "{path}");
        assert!(!path.starts_with("ignored/"), "{path}");
        assert_ne!(path, "src/old.bak.rs");
        assert_ne!(path, "excluded.rs");
    }
}

#[test]
fn walk_paths_are_relative_to_the_root() {
    let repo = FixtureRepo::new();
    let root = repo.root().to_string_lossy().into_owned();
    let got = walk(repo.root()).expect("walk");
    for file in &got.files {
        assert!(!file.path.starts_with('/'), "{}", file.path);
        assert!(!file.path.contains(&root), "{}", file.path);
        assert!(!file.path.contains('\\'), "{}", file.path);
        assert_eq!(file.abs_path, repo.root().join(&file.path));
    }
}

#[test]
fn walk_of_a_missing_root_is_an_error() {
    let repo = FixtureRepo::new();
    assert!(walk(&repo.root().join("no-such-dir")).is_err());
}

#[test]
fn secret_denylist_matches_names_not_paths() {
    for name in [
        ".env",
        ".env.production",
        "id_rsa",
        "id_rsa.pub",
        "tls.pem",
        "signing.key",
    ] {
        assert!(is_secret(name), "{name}");
    }
    for name in [
        "env.rs",
        "lib.rs",
        "keys.rs",
        "pem.rs",
        "valid_id.rs",
        "key",
    ] {
        assert!(!is_secret(name), "{name}");
    }
}
