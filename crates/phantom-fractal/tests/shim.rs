//! Integration tests for `scripts/phantom-fractal-ssh` using a fake `ssh` on `PATH`.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::{Repo, TempDir, history};

const SHIM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/phantom-fractal-ssh"
);

fn make_fake_ssh(dir: &Path, log: &Path) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let ssh = bin.join("ssh");
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display());
    fs::write(&ssh, script).unwrap();
    let mut perms = fs::metadata(&ssh).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&ssh, perms).unwrap();
    bin
}

fn run_shim(
    bin: &Path,
    envs: &[(&str, &str)],
    args: &[&str],
    clear_path: bool,
) -> (Output, Vec<String>) {
    let log = bin.parent().unwrap().join("ssh-argv.log");
    let _ = fs::remove_file(&log);
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(SHIM).args(args);
    if clear_path {
        cmd.env("PATH", bin);
    } else {
        let base_path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{base_path}", bin.display()));
    }
    cmd.env("PHANTOM_SSH_HOST", "example-host");
    cmd.env_remove("PHANTOM_LOCAL_REPO");
    cmd.env_remove("PHANTOM_REMOTE_REPO");
    cmd.env_remove("PHANTOM_REMOTE_DATA_DIR");
    cmd.env_remove("PHANTOM_REMOTE_BIN");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run shim");
    let logged = fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    (out, logged)
}

#[test]
fn quotes_metacharacters_and_refuses_backslashes_traversal_and_unmapped_paths() {
    let tmp = TempDir::new("shim-quote");
    let log = tmp.path().join("ssh-argv.log");
    let bin = make_fake_ssh(tmp.path(), &log);

    // Semicolons, spaces, subshells, and single quotes arrive single-quoted.
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_REMOTE_BIN", "/opt/phantom-fractal"),
            ("PHANTOM_REMOTE_DATA_DIR", "/var/data"),
        ],
        &["callers", "foo; rm -rf /", "a 'b' $(c)"],
        false,
    );
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(argv.len(), 5, "{argv:?}");
    assert_eq!(&argv[..4], ["-o", "BatchMode=yes", "--", "example-host"]);
    assert_eq!(
        argv[4],
        r#"PHANTOM_FRACTAL_DATA='/var/data' exec '/opt/phantom-fractal' 'callers' 'foo; rm -rf /' 'a '\''b'\'' $(c)'"#
    );

    // Backslash in arg, parent traversal, and unmapped local absolute --repo fail cleanly.
    for bad_args in [
        vec!["callers", r"a\b"],
        vec!["status", "--repo", "../escape"],
        vec!["status", "--repo", "/unmapped/local/path"],
    ] {
        let (out, argv) = run_shim(&bin, &[], &bad_args, false);
        assert!(!out.status.success(), "{bad_args:?}");
        assert!(argv.is_empty(), "{bad_args:?} invoked ssh: {argv:?}");
    }
}

#[test]
fn mapping_keyed_locality_guard_probes_local_and_fails_closed() {
    let tmp = TempDir::new("shim-guard");
    let log = tmp.path().join("ssh-argv.log");
    let bin = make_fake_ssh(tmp.path(), &log);
    let repo = Repo::new("shim-repo", &history());
    let local = repo.root.to_str().unwrap();
    let head = repo.rev("HEAD");
    let remote = "/remote/workspace/repo";

    // 1. Mapped local --repo path on clean git repo -> EXPECTED_HEAD=<head> EXPECTED_DIRTY='0'
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_LOCAL_REPO", local),
            ("PHANTOM_REMOTE_REPO", remote),
        ],
        &["status", "--repo", local],
        false,
    );
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        argv[4],
        format!(
            "PHANTOM_EXPECTED_HEAD='{head}' PHANTOM_EXPECTED_DIRTY='0' exec 'phantom-fractal' 'status' '--repo' '{remote}'"
        )
    );

    // 2. Direct remote path --repo=/remote/workspace/repo/sub also probes PHANTOM_LOCAL_REPO!
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_LOCAL_REPO", local),
            ("PHANTOM_REMOTE_REPO", remote),
        ],
        &["find", "fa", &format!("--repo={remote}/sub")],
        false,
    );
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        argv[4].starts_with(&format!(
            "PHANTOM_EXPECTED_HEAD='{head}' PHANTOM_EXPECTED_DIRTY='0' exec "
        )),
        "{}",
        argv[4]
    );
    assert!(
        argv[4].contains(&format!("'--repo={remote}/sub'")),
        "{}",
        argv[4]
    );

    // 3. Missing git on PATH fails closed with PHANTOM_EXPECTED_DIRTY='1'.
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_LOCAL_REPO", local),
            ("PHANTOM_REMOTE_REPO", remote),
        ],
        &["status", "--repo", local],
        true,
    );
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        argv[4].starts_with("PHANTOM_EXPECTED_DIRTY='1' exec "),
        "{}",
        argv[4]
    );
    assert!(!argv[4].contains("PHANTOM_EXPECTED_HEAD="), "{}", argv[4]);

    // 4. Local dirty (untracked file) -> PHANTOM_EXPECTED_DIRTY='1'.
    fs::write(repo.root.join("scratch.rs"), "pub fn dirty() {}\n").unwrap();
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_LOCAL_REPO", local),
            ("PHANTOM_REMOTE_REPO", remote),
        ],
        &["status", "--repo", local],
        false,
    );
    assert!(out.status.success());
    assert!(
        argv[4].starts_with(&format!(
            "PHANTOM_EXPECTED_HEAD='{head}' PHANTOM_EXPECTED_DIRTY='1' exec "
        )),
        "{}",
        argv[4]
    );

    // 5. Non-git PHANTOM_LOCAL_REPO fails closed with PHANTOM_EXPECTED_DIRTY='1'.
    let non_git = TempDir::new("non-git");
    let (out, argv) = run_shim(
        &bin,
        &[
            ("PHANTOM_LOCAL_REPO", non_git.path().to_str().unwrap()),
            ("PHANTOM_REMOTE_REPO", remote),
        ],
        &["status", "--repo", remote],
        false,
    );
    assert!(out.status.success());
    assert!(
        argv[4].starts_with("PHANTOM_EXPECTED_DIRTY='1' exec "),
        "{}",
        argv[4]
    );
    assert!(!argv[4].contains("PHANTOM_EXPECTED_HEAD="), "{}", argv[4]);
}
