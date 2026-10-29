//! `mine` spawns nothing and reads no outside config (SPEC-w8 G1c, the
//! PM's macOS sandbox ruling).
//!
//! The test re-runs its own binary, this test only, under
//! `/usr/bin/sandbox-exec` with a profile that kills the process on any
//! exec or fork. The environment is poisoned (a fake `git` first on
//! `PATH`, `HOME`, `GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXEC_PATH`, `EDITOR`),
//! and so is the repository's own config (fsmonitor, hooks, editor, ssh,
//! textconv and filter drivers, an include). Every program named there
//! leaves a marker if it runs, every outside config file is unparseable,
//! and the object-directory variables point nowhere. The sandboxed scan must equal the
//! scan taken here, no marker may appear, and a control `/bin/sh` under
//! the same profile must be killed. On macOS a missing `sandbox-exec`
//! fails the test; on Linux the test is compiled out, and the G3a
//! fake-`git` test covers no-spawn.
#![cfg(target_os = "macos")]

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Command;

use phantom_fractal_git::{Window, mine};
use support::{DAY, HEAD, Op, Repo, Script, indexed};

const SANDBOX: &str = "/usr/bin/sandbox-exec";
const CHILD: &str = "PHANTOM_GIT_SANDBOX_ROOT";
const TEST: &str = "mine_under_an_exec_and_fork_kill_sandbox_equals_mine_outside";
const PATHS: [&str; 3] = ["a.rs", "b.rs", "c.rs"];

/// Allows everything except exec and fork, which kill the process; only
/// `program` itself may be exec'd, by `sandbox-exec`.
fn profile(program: &Path) -> String {
    format!(
        "(version 1)(allow default)\
         (deny process-exec (with send-signal SIGKILL))\
         (allow process-exec (literal \"{}\"))\
         (deny process-fork (with send-signal SIGKILL))",
        program.display()
    )
}

#[test]
fn mine_under_an_exec_and_fork_kill_sandbox_equals_mine_outside() {
    if let Some(root) = std::env::var_os(CHILD) {
        // The sandboxed run.
        let scan = mine(Path::new(&root), &Window::default(), &indexed(&PATHS));
        println!("SCAN {scan:?}");
        return;
    }
    assert!(
        Path::new(SANDBOX).exists(),
        "{SANDBOX} is required on macOS; this test does not skip"
    );

    let mut script = Script::default();
    for n in (0..4).rev() {
        let body = format!("{n}\n");
        script.commit(
            "main",
            HEAD - n * DAY,
            &[Op::Write("a.rs", &body), Op::Write("b.rs", &body)],
        );
    }
    script.commit("main", HEAD, &[Op::Write("c.rs", "1\n")]);
    script.commit("main", HEAD, &[Op::Write("c.rs", "2\n")]);
    let repo = Repo::new("sandbox", &script);

    // Marker programs, and config that names them.
    let bin = repo.dir().join("bin");
    let markers = repo.dir().join("markers");
    fs::create_dir_all(&bin).expect("bin dir");
    fs::create_dir_all(&markers).expect("markers dir");
    for name in [
        "git",
        "sh",
        "ssh",
        "vi",
        "editor",
        "fsmonitor",
        "filter",
        "textconv",
    ] {
        let program = bin.join(name);
        fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\n", markers.join(name).display()),
        )
        .expect("write marker program");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    // Unparseable: a run that loads any of it cannot open the repository.
    let unparseable = "[core\n\tbroken = \"\n";
    let included = repo.dir().join("included");
    fs::write(&included, unparseable).expect("include file");
    fs::write(repo.home.join(".gitconfig"), unparseable).expect("global config");
    let mut config = fs::read_to_string(repo.root.join(".git/config")).expect("config");
    config.push_str(&format!(
        "[core]\n\tfsmonitor = {b}/fsmonitor\n\thooksPath = {b}\n\teditor = {b}/editor\n\
         \tsshCommand = {b}/ssh\n[diff \"x\"]\n\ttextconv = {b}/textconv\n\
         [filter \"x\"]\n\tprocess = {b}/filter\n\tsmudge = {b}/filter\n\
         [include]\n\tpath = {i}\n",
        b = bin.display(),
        i = included.display()
    ));
    fs::write(repo.root.join(".git/config"), config).expect("poison config");
    fs::write(repo.root.join(".gitattributes"), "*.rs diff=x filter=x\n").expect("attributes");

    let expected = mine(&repo.root, &Window::default(), &indexed(&PATHS))
        .expect("mine")
        .expect("a git root despite the poisoned config");
    assert_eq!(expected.pairs.len(), 1, "{expected:?}");

    let exe = std::env::current_exe()
        .expect("test binary")
        .canonicalize()
        .expect("canonical test binary");
    let profile = profile(&exe);
    let output = Command::new(SANDBOX)
        .arg("-p")
        .arg(&profile)
        .arg(&exe)
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env_clear()
        .env(CHILD, &repo.root)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &repo.home)
        .env("GIT_DIR", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", repo.home.join(".gitconfig"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.repositoryformatversion")
        .env("GIT_CONFIG_VALUE_0", "9")
        .env("GIT_EXEC_PATH", &bin)
        .env("GIT_OBJECT_DIRECTORY", "/nonexistent")
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", "/nonexistent")
        .env("EDITOR", bin.join("editor"))
        .output()
        .expect("run sandbox-exec");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "sandboxed run failed ({:?}, signal {:?}): {stdout} {}",
        output.status.code(),
        output.status.signal(),
        String::from_utf8_lossy(&output.stderr)
    );
    let want = format!(
        "SCAN {:?}",
        Ok::<_, phantom_fractal_git::GitError>(Some(expected))
    );
    assert!(
        // libtest prints `test <name> ... ` on the same line first.
        stdout.lines().any(|line| line.ends_with(&want)),
        "sandboxed scan differs:\n{stdout}\nwant: {want}"
    );

    let control = Command::new(SANDBOX)
        .arg("-p")
        .arg(&profile)
        .args(["/bin/sh", "-c", "exit 0"])
        .env_clear()
        .status()
        .expect("run control");
    assert_eq!(control.signal(), Some(9), "control not killed: {control:?}");

    let ran: Vec<_> = fs::read_dir(&markers)
        .expect("markers")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(ran.is_empty(), "programs ran: {ran:?}");
}
