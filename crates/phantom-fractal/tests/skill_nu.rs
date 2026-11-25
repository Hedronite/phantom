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

    /// The exit code and the one envelope line `phantom.nu ARGS` prints,
    /// whose `command` is the first argument.
    fn run_with(&self, bin: &Bin, cwd: &Path, args: &[&str]) -> (i32, Value) {
        self.run_as(bin, cwd, args, args.first().copied().unwrap_or(""))
    }

    /// As `run_with`, for an envelope whose `command` is `command`. A
    /// success carries `exit: 0` as its last key.
    fn run_as(&self, bin: &Bin, cwd: &Path, args: &[&str], command: &str) -> (i32, Value) {
        let (code, envelope, _) = self.run_line(bin, cwd, args, command);
        (code, envelope)
    }

    /// As `run_as`, with the line as printed, keys in their printed order.
    fn run_line(
        &self,
        bin: &Bin,
        cwd: &Path,
        args: &[&str],
        command: &str,
    ) -> (i32, Value, String) {
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
        assert_eq!(envelope["command"], command, "{stdout}");
        if envelope["ok"] == true {
            assert!(
                stdout.trim_end().ends_with(r#","exit":0}"#),
                "a success ends with exit 0: {stdout}"
            );
        }
        (code, envelope, stdout)
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

    /// `data` of a successful run with the binary `bin`.
    fn ok_with(&self, bin: &Bin, args: &[&str]) -> Value {
        let (code, envelope) = self.run_with(bin, &self.repo, args);
        assert_eq!(
            (code, &envelope["ok"]),
            (0, &json!(true)),
            "{args:?}: {envelope}"
        );
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
        &["cochange", "--count", "--rust", "src/a.rs"],
        &["cochange", "--min-lift", "1", "src/a.rs"],
        &["cochange", "--count", "--min-lift=1", "src/a.rs"],
        &["map", "--budget", "big"],
        &["rank", "extra"],
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

/// A git history (`support::history`) indexed, and the skill run in its root.
fn history_fixture(nu: PathBuf) -> (Fixture, support::Repo) {
    let fx = Fixture::empty(nu, "skill-nu-git");
    let repo = support::Repo::new("skill-nu", &support::history());
    fx.index(&repo.root);
    (fx, repo)
}

fn partner_paths(data: &Value) -> Vec<&str> {
    data["partners"]
        .as_array()
        .expect("partners")
        .iter()
        .map(|p| p["path"].as_str().expect("path"))
        .collect()
}

#[test]
fn cochange_counts_every_tracked_file_by_default() {
    let Some(nu) = nu() else { return };
    let (fx, repo) = history_fixture(nu);
    let (tree, db) = (support::snapshot(&repo.root), support::database(&fx.data()));
    let at = |args: &[&str]| fx.run_with(&Bin::OnPath, &repo.root, args);
    let (code, envelope) = at(&["cochange", "src/a.rs"]);
    assert_eq!(code, 0, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["mode"], "count", "{data}");
    assert_eq!(
        (&data["scope"]["files"], &data["scope"]["weighting"]),
        (&json!("tracked"), &json!("none"))
    );
    assert!(
        data["note"].as_str().expect("note").contains("no decay"),
        "{data}"
    );
    // The first commit adds every file; then a shares 5 with b, 4 with c, 3 with g.
    let partners = data["partners"].as_array().expect("partners");
    let top = [("src/b.rs", 6), ("src/c.rs", 5), ("src/g.rs", 4)];
    let want: Vec<Value> = top
        .iter()
        .map(|(p, c)| json!({"path": p, "count": c}))
        .collect();
    assert_eq!(partners[..3], want[..], "{data}");
    assert!(
        partner_paths(data).contains(&"Cargo.toml"),
        "a tracked non-Rust file: {data}"
    );
    for partner in partners {
        assert_eq!(
            partner.as_object().expect("row").len(),
            2,
            "path and count only: {partner}"
        );
    }

    let (code, explicit) = at(&["cochange", "--count", "src/a.rs"]);
    assert_eq!(
        (code, &explicit["data"]),
        (0, data),
        "--count is the default"
    );

    let (code, manifest) = at(&["cochange", "Cargo.toml"]);
    assert_eq!(code, 0, "{manifest}");
    assert!(
        partner_paths(&manifest["data"]).contains(&"src/a.rs"),
        "{manifest}"
    );
    // A count for a file not tracked at HEAD is not_found, with no count note.
    let (code, untracked) = at(&["cochange", "src/nope.rs"]);
    assert_eq!(
        (code, untracked["kind"].as_str()),
        (3, Some("not_found")),
        "{untracked}"
    );
    assert!(untracked.get("note").is_none(), "{untracked}");

    let (code, one) = at(&["cochange", "--limit", "1", "src/a.rs"]);
    assert_eq!(
        (code, partner_paths(&one["data"])),
        (0, vec!["src/b.rs"]),
        "{one}"
    );
    assert_eq!(one["data"]["truncated"], true, "{one}");

    assert_eq!(
        support::snapshot(&repo.root),
        tree,
        "cochange wrote the tree"
    );
    assert_eq!(
        support::database(&fx.data()),
        db,
        "cochange wrote the index"
    );
}

#[test]
fn cochange_rust_is_the_labelled_decayed_mode() {
    let Some(nu) = nu() else { return };
    let (fx, repo) = history_fixture(nu);
    let (tree, db) = (support::snapshot(&repo.root), support::database(&fx.data()));
    let at = |args: &[&str]| fx.run_with(&Bin::OnPath, &repo.root, args);
    let (code, envelope) = at(&["cochange", "--rust", "src/a.rs"]);
    assert_eq!(code, 0, "{envelope}");
    let data = &envelope["data"];
    assert_eq!(data["mode"], "rust", "{data}");
    assert_eq!(data["scope"]["files"], "indexed .rs", "{data}");
    assert!(
        data["note"]
            .as_str()
            .expect("note")
            .contains("not a commit count"),
        "{data}"
    );
    let partners = data["partners"].as_array().expect("partners");
    assert!(!partners.is_empty(), "{data}");
    for partner in partners {
        assert!(
            partner["path"].as_str().is_some_and(|p| p.ends_with(".rs")),
            "{partner}"
        );
        assert!(partner["together_decayed"].as_f64().is_some(), "{partner}");
        assert!(partner["lift"].as_f64().is_some(), "{partner}");
        assert!(
            partner["last_time_iso"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z')),
            "{partner}"
        );
    }
    let (code, lifted) = at(&["cochange", "--rust", "--min-lift", "0", "src/a.rs"]);
    assert_eq!(code, 0, "{lifted}");

    let (code, envelope) = at(&["cochange", "--rust", "Cargo.toml"]);
    assert_eq!(
        (code, envelope["kind"].as_str()),
        (3, Some("not_found")),
        "{envelope}"
    );
    assert!(
        envelope["note"]
            .as_str()
            .is_some_and(|n| n.contains("drop --rust")),
        "{envelope}"
    );

    assert_eq!(
        support::snapshot(&repo.root),
        tree,
        "cochange --rust wrote the tree"
    );
    assert_eq!(
        support::database(&fx.data()),
        db,
        "cochange --rust wrote the index"
    );
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

/// The first word of each usage `help` lists, in order.
const HELP_COMMANDS: &str =
    "where callers callees impact diff-impact path cochange map rank show status index";

#[test]
fn help_prints_the_command_table_and_runs_nothing() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    // A run would be `missing_phantom`, exit 127: help proves none started.
    let nowhere = Bin::Override(fx.tmp.path().join("nowhere"));
    let (code, table) = fx.run_as(&nowhere, &fx.repo, &["help"], "help");
    assert_eq!((code, &table["ok"]), (0, &json!(true)), "{table}");
    let usages: Vec<&str> = table["data"]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|row| row["usage"].as_str().expect("usage"))
        .collect();
    let mut heads: Vec<&str> = usages
        .iter()
        .map(|usage| usage.split(' ').next().expect("a word"))
        .collect();
    heads.dedup();
    assert_eq!(heads.join(" "), HELP_COMMANDS, "{table}");
    assert!(
        usages.contains(&"impact NAME [--depth 1..5] [--limit N] [--no-tests]"),
        "{table}"
    );
    for key in ["flags", "names", "envelope", "read"] {
        assert!(table["data"][key].as_str().is_some(), "{key}: {table}");
    }
    // One line a harness raised to 8192 columns shows whole.
    assert!(table.to_string().len() < 8192, "{table}");

    for args in [
        &["--help"][..],
        &["-h"],
        &["callers", "--help"],
        &["impact", "x", "-h"],
        &["status", "--help"],
        &["--repo", "elsewhere", "--help"],
        &["help", "where"],
    ] {
        let (code, again) = fx.run_as(&nowhere, &fx.repo, args, "help");
        assert_eq!((code, &again), (0, &table), "{args:?}");
    }
    // After `--`, `--help` is a name.
    fx.fails(&Bin::OnPath, &["callers", "--", "--help"], "not_found", 3);
}

#[test]
fn global_flags_may_come_before_the_command() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    let repo = fx.repo.to_str().expect("utf-8");
    let joined = format!("--repo={repo}");
    let elsewhere = fx.tmp.path().join("home");
    let pairs: [(&[&str], &[&str]); 5] = [
        (
            &["--repo", repo, "where", "build"],
            &["where", "--repo", repo, "build"],
        ),
        (&[&joined, "status"], &["status", &joined]),
        (
            &["--repo", repo, "--limit", "1", "callers", "fixture_data"],
            &["callers", "fixture_data", "--limit", "1", "--repo", repo],
        ),
        (
            &[
                "--depth",
                "2",
                "--repo",
                repo,
                "impact",
                "--no-tests",
                "build",
            ],
            &[
                "impact",
                "--no-tests",
                "--depth",
                "2",
                "--repo",
                repo,
                "build",
            ],
        ),
        (
            &["--repo", repo, "path", "entry", "build"],
            &["path", "entry", "build", "--repo", repo],
        ),
    ];
    for (before, after) in pairs {
        let command = after[0];
        let (code, lead) = fx.run_as(&Bin::OnPath, &elsewhere, before, command);
        let (again, tail) = fx.run_as(&Bin::OnPath, &elsewhere, after, command);
        assert_eq!((code, again), (0, 0), "{lead}\n{tail}");
        assert_eq!(lead, tail, "{before:?}");
    }

    // Usage errors run nothing: a run would be `missing_phantom`.
    let nowhere = Bin::Override(fx.tmp.path().join("nowhere"));
    for (args, command) in [
        (&["--depth", "2", "where", "build"][..], "where"),
        (&["--limit", "1", "bogus"], "bogus"),
        (&["--repo"], ""),
        (&["--repo", repo], ""),
        (&["--bogus", "where", "build"], ""),
        (&["--", "where", "build"], ""),
    ] {
        let (code, envelope) = fx.run_as(&nowhere, &fx.repo, args, command);
        assert_eq!(
            (code, envelope["kind"].as_str()),
            (1, Some("usage")),
            "{args:?}: {envelope}"
        );
    }
}

/// A copy of `fixtures/impact`, indexed: `root_fn` has 5 callers at depth
/// 1, 2 at depth 2, 1 at depth 3, and 2 tests.
fn impact_fixture(nu: PathBuf) -> Fixture {
    let fixture = Fixture::empty(nu, "skill-nu-impact");
    support::copy_fixture("impact", &fixture.repo);
    fixture.index(&fixture.repo);
    fixture
}

fn totals(data: &Value) -> Vec<&Value> {
    data["by_depth"]
        .as_array()
        .expect("by_depth")
        .iter()
        .map(|group| &group["total"])
        .collect()
}

fn kept(data: &Value) -> usize {
    data["by_depth"]
        .as_array()
        .expect("by_depth")
        .iter()
        .map(|group| group["callers"].as_array().expect("callers").len())
        .sum()
}

#[test]
fn impact_limit_passes_through_and_counts_what_it_cut() {
    let Some(nu) = nu() else { return };
    let fx = impact_fixture(nu);
    let all = fx.ok(&["impact", "--no-tests", "root_fn"]);
    assert!(all.get("limit").is_none(), "no limit, no key: {all}");
    assert_eq!(kept(&all), 8, "{all}");

    let cut = fx.ok(&["impact", "--no-tests", "--limit", "2", "root_fn"]);
    assert_eq!(totals(&cut), totals(&all), "cut groups keep their totals");
    assert_eq!(kept(&cut), 2, "{cut}");
    assert_eq!(
        displays(&cut["by_depth"][0]["callers"]),
        ["app/src/main.rs::run", "app/src/main.rs::report"],
        "the shallowest first"
    );
    for group in cut["by_depth"].as_array().expect("by_depth") {
        assert_eq!(group["truncated"], true, "{group}");
    }
    assert_eq!(
        cut["limit"],
        json!({"max": 2, "omitted_hits": 6, "omitted_tests": 0})
    );
    let (code, lead) = fx.run_as(
        &Bin::OnPath,
        &fx.repo,
        &["--limit", "2", "impact", "--no-tests", "root_fn"],
        "impact",
    );
    assert_eq!(
        (code, &lead["data"]),
        (0, &cut),
        "--limit before the command"
    );

    let (_, _, line) = fx.run_line(
        &Bin::OnPath,
        &fx.repo,
        &["impact", "--limit", "2", "root_fn"],
        "impact",
    );
    assert!(
        line.contains(r#""files":["app/src/main.rs","engine/src/tests.rs"],"limit":{"#),
        "limit follows the W14 keys: {line}"
    );

    let tests = fx.ok(&["impact", "--limit=1", "root_fn"]);
    assert_eq!(tests["tests"].as_array().map(Vec::len), Some(1), "{tests}");
    assert_eq!(tests["limit"]["omitted_tests"], 1, "{tests}");

    // The skill refuses a non-number and runs nothing; the CLI's range is its own.
    let nowhere = Bin::Override(fx.tmp.path().join("nowhere"));
    for args in [
        &["impact", "--limit", "0", "root_fn"][..],
        &["impact", "--limit", "two", "root_fn"],
        &["diff-impact", "--limit", "1"],
    ] {
        fx.fails(&nowhere, args, "usage", 1);
    }
    let over = fx.ok(&["impact", "--limit", "200", "root_fn"]);
    assert_eq!(
        (&over["truncated"], &over["limit"]["max"]),
        (&json!(true), &json!(100))
    );
}

#[test]
fn a_display_from_any_reply_is_a_name() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    let ambiguous = fx.fails(&Bin::OnPath, &["callers", "twin"], "ambiguous", 6);
    let listed = fx.ok(&["where", "twin"]);
    let mut seen = Vec::new();
    for (matched, hit) in ambiguous["matches"]
        .as_array()
        .expect("matches")
        .iter()
        .zip(listed["hits"].as_array().expect("hits"))
    {
        assert_eq!(matched["name"], hit["name"], "the same order");
        for display in [&matched["display"], &hit["display"]] {
            let name = display.as_str().expect("display");
            for command in ["callers", "impact", "show"] {
                let data = fx.ok(&[command, name]);
                assert_eq!(data["symbol"]["name"], matched["name"], "{command} {name}");
            }
            let again = fx.ok(&["where", name]);
            assert_eq!(again["hits"][0]["display"], hit["display"], "{name}");
            seen.push(name.to_owned());
        }
    }
    assert_eq!(
        seen,
        [
            "src/alt.rs::twin",
            "src/alt.rs:1 twin",
            "src/model.rs::twin",
            "src/model.rs:5 twin"
        ]
    );
    let data = fx.ok(&["where", "src/model.rs:14 Gadget::new"]);
    assert_eq!(data["hits"][0]["name"], "crate::model::Gadget::new");
    let data = fx.ok(&["path", "src/lib.rs:15 entry", "src/model.rs::build"]);
    assert_eq!(data["route"], "src/lib.rs::entry -> src/model.rs::build");
    // A line that names nothing there is still not found.
    fx.fails(
        &Bin::OnPath,
        &["callers", "src/alt.rs:1 nope"],
        "not_found",
        3,
    );
}

#[test]
fn a_reexported_target_carries_the_grep_flag() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::dogfood(nu);
    // `mid` and `prelude` both `pub use` the model's `Gadget`.
    for command in ["callers", "impact"] {
        let (code, envelope, line) =
            fx.run_line(&Bin::OnPath, &fx.repo, &[command, "Gadget"], command);
        assert_eq!(code, 0, "{envelope}");
        assert_eq!(
            (
                &envelope["data"]["reexported"],
                &envelope["data"]["confirm_with_grep"]
            ),
            (&json!(true), &json!(true)),
            "{envelope}"
        );
        assert!(
            line.contains(r#","reexported":true,"confirm_with_grep":true},"meta":"#),
            "the flags end data: {line}"
        );
        for name in ["build", "fixture_data"] {
            let data = fx.ok(&[command, name]);
            assert!(data.get("reexported").is_none(), "{command} {name}: {data}");
            assert!(data.get("confirm_with_grep").is_none(), "{data}");
        }
    }
    let data = fx.ok(&["callees", "entry"]);
    assert!(data.get("reexported").is_none(), "{data}");
}

/// A fake `phantom-fractal` that logs its argv and prints `reply`. It uses
/// only shell builtins: the skill runs with an empty `PATH`.
#[cfg(unix)]
fn fake(fx: &Fixture, reply: &str) -> (Bin, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (bin, log) = (fx.tmp.path().join("fake"), fx.tmp.path().join("argv"));
    let text = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf '%s\\n' '{reply}'\n",
        log.display()
    );
    fs::write(&bin, text).expect("fake");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::create_dir_all(&fx.repo).expect("repo");
    (Bin::Override(bin), log)
}

fn argv(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A reply every read command can reshape.
const FAKE_REPLY: &str = r#"{"ok":true,"data":{"symbol":{"name":"crate::f","path":"src/a.rs","line":1},"hits":[]},"meta":null}"#;

#[cfg(unix)]
#[test]
fn no_read_command_sends_index() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::empty(nu, "skill-nu-reads");
    let (bin, log) = fake(&fx, FAKE_REPLY);
    let reads = "where f;callers f;callees f;impact f;diff-impact;path a b;cochange src/a.rs;\
                 cochange --rust src/a.rs;map;rank;show f;status";
    for read in reads.split(';') {
        let args: Vec<&str> = read.split(' ').collect();
        let (code, envelope) = fx.run_with(&bin, &fx.repo, &args);
        assert_eq!(code, 0, "{args:?}: {envelope}");
    }
    let sent = argv(&log);
    assert_eq!(sent.len(), 12, "{sent:?}");
    let reads_only = |line: &String| line.starts_with("--json ") && !line.contains(" index");
    assert!(sent.iter().all(reads_only), "{sent:?}");
}

/// `index` runs at any time (A4-w15), with one path and no flag.
#[cfg(unix)]
#[test]
fn index_takes_one_path_and_no_flag() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::empty(nu, "skill-nu-index-argv");
    let (bin, log) = fake(&fx, FAKE_REPLY);
    for args in [
        &["index", "a", "b"][..],
        &["index", "--force"],
        &["index", "--repo", "x"],
        &["index", "--", "a", "b"],
    ] {
        fx.fails(&bin, args, "usage", 1);
    }
    assert!(argv(&log).is_empty(), "a usage error ran the CLI");
    fx.ok_with(&bin, &["index"]);
    fx.ok_with(&bin, &["index", "src"]);
    assert_eq!(argv(&log), ["--json index -- .", "--json index -- src"]);
}

#[cfg(unix)]
#[test]
fn where_answers_an_envelope_when_show_has_no_symbol() {
    let Some(nu) = nu() else { return };
    let fx = Fixture::empty(nu, "skill-nu-nosym");
    for reply in [
        r#"{"ok":true,"data":{},"meta":null}"#,
        r#"{"ok":true,"data":null,"meta":null}"#,
        r#"{"ok":true,"data":{"symbol":"f"},"meta":null}"#,
    ] {
        let (bin, _) = fake(&fx, reply);
        let envelope = fx.fails(&bin, &["where", "f"], "error", 1);
        assert!(
            envelope["error"]
                .as_str()
                .expect("error")
                .contains("without a symbol"),
            "{envelope}"
        );
    }
}
