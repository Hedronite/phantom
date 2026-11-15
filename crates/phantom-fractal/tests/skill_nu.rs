//! Run the `phantom` skill's `phantom.nu` against the built binary
//! (SPEC-w14 G2a), on a copy of `fixtures/resolve-dogfood` indexed into a
//! scratch data directory, with a scratch home and a cleared environment.
//!
//! Nushell comes from `PHANTOM_NU`, else `nu` on `PATH`. It is never
//! downloaded: without it each test prints why and passes (D5). A
//! `PHANTOM_NU` that names no file is an error, not a skip.

mod support;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};
use support::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_phantom-fractal");

/// The Nushell to run, or None after printing why the test is skipped.
fn nu() -> Option<PathBuf> {
    if let Some(over) = std::env::var_os("PHANTOM_NU").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(over);
        assert!(path.is_file(), "PHANTOM_NU does not name a file");
        return Some(path);
    }
    let found = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("nu"))
                .find(|nu| nu.is_file())
        })
        .unwrap_or_default();
    if found.is_none() {
        eprintln!("skill_nu: skipped: no nu on PATH and no PHANTOM_NU; nu is never downloaded");
    }
    found
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.agents/skills/phantom/phantom.nu")
}

/// Where the skill finds the binary.
enum Bin {
    /// `phantom-fractal` on `PATH`.
    OnPath,
    /// `PATH` is an empty directory and the override names this.
    Override(PathBuf),
    /// `PATH` is an empty directory and there is no override.
    Missing,
}

/// A scratch home, data directory and fixture copy.
struct Fixture {
    nu: PathBuf,
    tmp: TempDir,
    repo: PathBuf,
}

impl Fixture {
    /// A copy of `fixtures/resolve-dogfood`, indexed.
    fn dogfood(nu: PathBuf) -> Self {
        let fixture = Self::empty(nu, "skill-nu");
        support::copy_fixture("resolve-dogfood", &fixture.repo);
        fixture.index(&fixture.repo);
        fixture
    }

    fn empty(nu: PathBuf, label: &str) -> Self {
        let tmp = TempDir::new(label);
        let repo = tmp.path().join("repo");
        for dir in ["home/config", "home/cache", "data", "empty-path"] {
            fs::create_dir_all(tmp.path().join(dir)).expect("scratch dir");
        }
        Self { nu, tmp, repo }
    }

    fn data(&self) -> PathBuf {
        self.tmp.path().join("data")
    }

    fn index(&self, root: &Path) {
        let mut cmd = Command::new(BIN);
        cmd.args(["--json", "index", "."]).current_dir(root);
        self.isolate(&mut cmd, OsString::from(self.tmp.path().join("empty-path")));
        let out = cmd.output().expect("run index");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    fn isolate(&self, cmd: &mut Command, path: OsString) {
        let home = self.tmp.path().join("home");
        cmd.env_clear()
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("PHANTOM_FRACTAL_DATA", self.data())
            .env("PATH", path);
        // An instrumented binary writes its profile here. Without it the
        // profile lands in the working directory, which is the fixture.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
    }

    /// The exit code and the one envelope line `phantom.nu ARGS` prints.
    fn run_with(&self, bin: &Bin, cwd: &Path, args: &[&str]) -> (i32, Value) {
        let mut cmd = Command::new(&self.nu);
        cmd.arg("--no-config-file")
            .arg(script())
            .args(args)
            .current_dir(cwd);
        let empty = OsString::from(self.tmp.path().join("empty-path"));
        match bin {
            Bin::OnPath => {
                self.isolate(&mut cmd, Path::new(BIN).parent().expect("bin dir").into());
            }
            Bin::Override(path) => {
                self.isolate(&mut cmd, empty);
                cmd.env("PHANTOM_FRACTAL_BIN", path);
            }
            Bin::Missing => self.isolate(&mut cmd, empty),
        }
        let out = cmd.output().expect("run nu");
        let stdout = String::from_utf8(out.stdout).expect("utf-8");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            stdout.lines().count(),
            1,
            "one line {args:?}\n{stdout}\n{stderr}"
        );
        let envelope: Value = serde_json::from_str(&stdout).expect("one JSON envelope");
        let code = out.status.code().expect("exit code");
        assert_eq!(
            envelope["exit"].as_i64().unwrap_or(0),
            i64::from(code),
            "{stdout}"
        );
        assert_eq!(
            envelope["command"],
            args.first().copied().unwrap_or(""),
            "{stdout}"
        );
        (code, envelope)
    }

    fn run(&self, args: &[&str]) -> (i32, Value) {
        self.run_with(&Bin::OnPath, &self.repo, args)
    }

    /// `data` of a successful run.
    fn ok(&self, args: &[&str]) -> Value {
        let (code, envelope) = self.run(args);
        assert_eq!(code, 0, "{args:?}: {envelope}");
        assert_eq!(envelope["ok"], true, "{envelope}");
        envelope["data"].clone()
    }

    /// A failed run of `kind`, exiting `code`.
    fn fails(&self, bin: &Bin, args: &[&str], kind: &str, code: i32) -> Value {
        let (got, envelope) = self.run_with(bin, &self.repo, args);
        assert_eq!(
            (got, envelope["kind"].as_str()),
            (code, Some(kind)),
            "{args:?}: {envelope}"
        );
        assert_eq!(envelope["ok"], false, "{envelope}");
        assert!(
            envelope["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{envelope}"
        );
        envelope
    }
}

fn displays(rows: &Value) -> Vec<&str> {
    rows.as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            row.as_str()
                .or_else(|| row["display"].as_str())
                .expect("display")
        })
        .collect()
}

fn names(rows: &Value) -> Vec<&str> {
    let mut out: Vec<&str> = rows
        .as_array()
        .expect("candidates")
        .iter()
        .map(|row| row["name"].as_str().expect("name"))
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn where_finds_a_method_and_lists_every_candidate() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    for name in ["Gadget::new", "new", "model::new", "model::Gadget::new"] {
        let data = fx.ok(&["where", name]);
        assert_eq!(data["total"], 1, "{name}: {data}");
        let hit = &data["hits"][0];
        assert_eq!(hit["display"], "src/model.rs:14 Gadget::new", "{name}");
        assert_eq!((&hit["kind"], &hit["line"]), (&json!("method"), &json!(14)));
        assert_eq!(hit["signature"], "pub fn new() -> Gadget");
    }
    let twin = fx.ok(&["where", "twin"]);
    assert_eq!(twin["total"], 2);
    assert_eq!(
        displays(&twin["hits"]),
        ["src/alt.rs:1 twin", "src/model.rs:5 twin"]
    );
    fx.fails(&Bin::OnPath, &["where", "nope"], "not_found", 3);
}

#[test]
fn callers_and_callees_group_sites_and_list_files() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    let data = fx.ok(&["callers", "fixture_data"]);
    assert_eq!(
        data["symbol"]["display"],
        "tests/common/mod.rs::fixture_data"
    );
    let callers = &data["callers"];
    assert_eq!(
        displays(callers),
        ["tests/first.rs::first_case", "tests/second.rs::second_case"]
    );
    assert_eq!(callers[0]["sites"], json!([5]));
    assert_eq!(callers[1]["sites"], json!([7, 8]));
    assert_eq!(callers[1]["min_confidence"], 0.85);
    assert_eq!(callers[1]["reasons"], json!(["local", "use-glob"]));
    assert_eq!(data["files"], json!(["tests/first.rs", "tests/second.rs"]));
    assert_eq!(
        (&data["site_count"], &data["truncated"]),
        (&json!(3), &json!(false))
    );

    let one = fx.ok(&["callers", "--limit", "1", "fixture_data"]);
    assert_eq!(displays(&one["callers"]), ["tests/first.rs::first_case"]);
    assert_eq!(
        (&one["truncated"], &one["site_count"]),
        (&json!(true), &json!(3))
    );

    let data = fx.ok(&["callees", "entry"]);
    assert_eq!(
        displays(&data["callees"]),
        [
            "src/lib.rs::local_wins",
            "src/model.rs::Gadget::new",
            "src/model.rs::build"
        ]
    );
    assert_eq!(data["callees"][2]["sites"], json!([17, 20]));
    assert_eq!(data["files"], json!(["src/lib.rs", "src/model.rs"]));
    assert_eq!(
        (&data["site_count"], &data["unresolved_calls"]),
        (&json!(4), &json!(2))
    );
}

#[test]
fn ambiguous_and_not_found_carry_their_candidates() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    let twins = ["crate::alt::twin", "crate::model::twin"];
    for command in ["callers", "callees", "impact", "show"] {
        let envelope = fx.fails(&Bin::OnPath, &[command, "twin"], "ambiguous", 6);
        assert_eq!(names(&envelope["matches"]), twins, "{command}: {envelope}");
        assert_eq!(envelope["total"], 2);
    }
    for command in ["callers", "where"] {
        let envelope = fx.fails(&Bin::OnPath, &[command, "nope::twin"], "not_found", 3);
        assert_eq!(
            names(&envelope["did_you_mean"]),
            twins,
            "{command}: {envelope}"
        );
    }
    let envelope = fx.fails(&Bin::OnPath, &["callers", "no_such_name"], "not_found", 3);
    assert!(envelope.get("matches").is_none(), "{envelope}");
}

#[test]
fn usage_errors_are_envelopes_and_run_nothing() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    // A run would be `missing_phantom`, exit 127: usage proves none started.
    let nowhere = Bin::Override(fx.tmp.path().join("nowhere"));
    for args in [
        &[][..],
        &["bogus", "x"],
        &["callers"],
        &["callers", "a", "b"],
        &["callers", "--bogus", "x"],
        &["callers", "--", "--repo"],
        &["callers", "--repo"],
        &["callers", "--limit", "abc", "x"],
        &["callers", "--limit=0", "x"],
        &["impact", "--depth", "two", "x"],
        &["diff-impact", "x"],
        &["path", "entry"],
        &["cochange"],
        &["map", "--budget", "big"],
        &["rank", "extra"],
        &["status", "--help"],
        &["index", "a", "b"],
    ] {
        fx.fails(&nowhere, args, "usage", 1);
    }
    // A CLI usage error, after the run: clap's text becomes an envelope.
    fx.fails(
        &Bin::OnPath,
        &["impact", "--depth", "9", "fixture_data"],
        "usage",
        1,
    );
    // A dash-leading name after `--` reaches the CLI as a name.
    fx.fails(&Bin::OnPath, &["callers", "--", "-x"], "not_found", 3);
}

#[test]
fn a_missing_binary_is_exit_127_and_the_override_is_used() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    for bin in [Bin::Missing, Bin::Override(fx.tmp.path().join("nowhere"))] {
        let envelope = fx.fails(&bin, &["status"], "missing_phantom", 127);
        assert_eq!(envelope["exit"], 127);
    }
    let (code, envelope) = fx.run_with(&Bin::Override(BIN.into()), &fx.repo, &["status"]);
    assert_eq!(code, 0, "{envelope}");
    assert_eq!(envelope["data"]["stale"], false);
    assert_eq!(envelope["meta"]["stale"], false);
}

#[test]
fn the_other_reads_reshape_and_write_nothing() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    let (tree, db) = (support::snapshot(&fx.repo), support::database(&fx.data()));

    let data = fx.ok(&["impact", "fixture_data"]);
    assert_eq!(
        data["tests"],
        json!(["tests/first.rs::first_case", "tests/second.rs::second_case"])
    );
    assert_eq!(data["files"], json!(["tests/first.rs", "tests/second.rs"]));
    let data = fx.ok(&["impact", "--depth", "2", "--no-tests", "build"]);
    assert_eq!(data["by_depth"][0]["callers"], json!(["src/lib.rs::entry"]));
    assert_eq!(data["symbol"]["display"], "src/model.rs::build");

    let data = fx.ok(&["path", "entry", "build"]);
    assert_eq!(data["route"], "src/lib.rs::entry -> src/model.rs::build");
    assert_eq!(data["found"], true);

    let data = fx.ok(&["rank", "--limit", "1"]);
    assert_eq!(displays(&data["hits"]), ["src/model.rs::build"]);
    let data = fx.ok(&["rank", "--files", "--limit", "1"]);
    assert_eq!(data["hits"][0]["path"], "src/model.rs");

    let data = fx.ok(&["map", "--budget", "200", "--focus", "src/lib.rs"]);
    assert!(
        data["text"].as_str().expect("text").contains("src/lib.rs"),
        "{data}"
    );

    let data = fx.ok(&["show", "Gadget::new"]);
    assert_eq!(data["symbol"]["display"], "src/model.rs:14 Gadget::new");
    assert_eq!(displays(&data["callers"]["callers"]), ["src/lib.rs::entry"]);
    assert_eq!(data["callers"]["count"], 1);

    let data = fx.ok(&["status"]);
    assert_eq!(
        (&data["stale"], &data["lock"]),
        (&json!(false), &json!("free"))
    );

    // Off a git root `impact --diff` is the CLI's own usage envelope.
    fx.fails(&Bin::OnPath, &["diff-impact"], "usage", 1);
    let elsewhere = fx.tmp.path().join("home");
    let repo = fx.repo.to_str().expect("utf-8");
    let (code, envelope) = fx.run_with(
        &Bin::OnPath,
        &elsewhere,
        &["where", "--repo", repo, "build"],
    );
    assert_eq!(
        (code, &envelope["data"]["total"]),
        (0, &json!(1)),
        "{envelope}"
    );

    assert_eq!(support::snapshot(&fx.repo), tree, "a read wrote the tree");
    assert_eq!(support::database(&fx.data()), db, "a read wrote the index");
}

#[test]
fn cochange_keeps_its_scope_note() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::empty(nu, "skill-nu-git");
    let repo = support::Repo::new("skill-nu", &support::history());
    fx.index(&repo.root);
    let (code, envelope) = fx.run_with(&Bin::OnPath, &repo.root, &["cochange", "src/a.rs"]);
    assert_eq!(code, 0, "{envelope}");
    let data = &envelope["data"];
    assert!(
        data["scope"]
            .as_str()
            .expect("scope")
            .contains("not a raw commit count")
    );
    let partners = data["partners"].as_array().expect("partners");
    assert!(!partners.is_empty(), "{data}");
    for partner in partners {
        assert!(partner["together_decayed"].as_f64().is_some(), "{partner}");
        assert!(partner["lift"].as_f64().is_some(), "{partner}");
        assert!(
            partner["last_time_iso"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z')),
            "{partner}"
        );
    }
    let (code, envelope) = fx.run_with(&Bin::OnPath, &repo.root, &["cochange", "README.md"]);
    assert_eq!(
        (code, envelope["kind"].as_str()),
        (3, Some("not_found")),
        "{envelope}"
    );
    assert!(envelope["scope"].as_str().is_some(), "{envelope}");
}

#[test]
fn index_runs_through_the_skill() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::empty(nu, "skill-nu-index");
    support::copy_fixture("resolve-dogfood", &fx.repo);
    let tree = support::snapshot(&fx.repo);
    let data = fx.ok(&["index"]);
    assert_eq!(
        (&data["written"], &data["edges"]),
        (&json!(9), &json!(7)),
        "{data}"
    );
    let again = fx.ok(&["index", "."]);
    assert_eq!(again["written"], 0, "{again}");
    assert_eq!(support::snapshot(&fx.repo), tree, "index wrote the tree");
}
