//! CLI resolution and graph queries over nested inner `fn` items in
//! `fixtures/resolve-nested` (SPEC-w16 G2).

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

mod support;

use support::{TempDir, copy_fixture};

const BINS: [&str; 2] = [
    env!("CARGO_BIN_EXE_phantom-fractal"),
    env!("CARGO_BIN_EXE_fract"),
];

struct World {
    repo: TempDir,
    data: TempDir,
}

impl World {
    fn new() -> Self {
        let world = Self {
            repo: TempDir::new("nested-repo"),
            data: TempDir::new("nested-data"),
        };
        copy_fixture("resolve-nested", world.repo.path());
        let (code, env) = world.run(BINS[0], &["index", "."]);
        assert_eq!(code, Some(0), "{env}");
        world
    }

    fn run(&self, bin: &str, args: &[&str]) -> (Option<i32>, Value) {
        let out = Command::new(bin)
            .args(args)
            .env("PHANTOM_FRACTAL_DATA", self.data.path())
            .current_dir(self.repo.path())
            .output()
            .expect("run");
        let text = String::from_utf8(out.stdout).expect("utf-8");
        let env: Value = serde_json::from_str(text.trim_end()).expect("json");
        (out.status.code(), env)
    }
}

#[test]
fn nested_fns_resolve_via_find_show_alias_and_path_line() {
    let world = World::new();
    for bin in BINS {
        let (code, find_env) = world.run(bin, &["find", "is_root"]);
        assert_eq!(code, Some(0), "{bin} find: {find_env}");
        assert_eq!(find_env["data"]["hits"][0]["name"], "is_root", "{find_env}");
        assert_eq!(find_env["data"]["hits"][0]["line"], 6, "{find_env}");

        for query in [
            "is_root",
            "nearby_entries::is_root",
            "app::nearby_entries::is_root",
            "crate::app::nearby_entries::is_root",
            "resolve_nested::app::nearby_entries::is_root",
            "resolve-nested::app::nearby_entries::is_root",
            "app::is_root",
            "src/app.rs::nearby_entries::is_root",
            "src/app.rs:6",
        ] {
            let (code, env) = world.run(bin, &["show", query]);
            assert_eq!(code, Some(0), "{bin} show {query}: {env}");
            assert_eq!(
                env["data"]["symbol"]["qual_name"], "crate::app::nearby_entries::is_root",
                "{query}: {env}"
            );
            assert_eq!(env["data"]["symbol"]["path"], "src/app.rs", "{query}");
            assert_eq!(env["data"]["symbol"]["line"], 6, "{query}");
        }

        let (code, env) = world.run(bin, &["show", "app::nearby_entries::is_root"]);
        assert_eq!(code, Some(0), "{bin} show: {env}");
        assert_eq!(env["data"]["symbol"]["line"], 6, "{env}");
        assert_eq!(env["data"]["symbol"]["end_line"], 9, "{env}");
        assert_eq!(
            env["data"]["symbol"]["doc_summary"], "Returns whether `path` is root.",
            "{env}"
        );

        let (code, env) = world.run(bin, &["show", "App::run::step"]);
        assert_eq!(code, Some(0), "{bin} show step: {env}");
        assert_eq!(
            env["data"]["symbol"]["qual_name"],
            "crate::app::App::run::step"
        );
        assert_eq!(env["data"]["symbol"]["line"], 21);

        let (code, env) = world.run(bin, &["show", "layered::mid::leaf"]);
        assert_eq!(code, Some(0), "{bin} show leaf: {env}");
        assert_eq!(
            env["data"]["symbol"]["qual_name"],
            "crate::layered::mid::leaf"
        );
        assert_eq!(env["data"]["symbol"]["line"], 21);
    }
}

#[test]
fn same_named_nested_fns_are_ambiguous_bare_and_scoped_for_graph_queries() {
    let world = World::new();
    for bin in BINS {
        let (code, find_shared) = world.run(bin, &["find", "shared_helper"]);
        assert_eq!(code, Some(0), "{bin} find shared_helper: {find_shared}");
        assert_eq!(
            find_shared["data"]["hits"].as_array().expect("hits").len(),
            2
        );

        let (code, env) = world.run(bin, &["show", "shared_helper"]);
        assert_eq!(code, Some(6), "{bin} ambiguous show: {env}");
        assert_eq!(env["error"]["kind"], "ambiguous", "{env}");
        let displays: Vec<&str> = env["data"]["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .map(|c| c["display"].as_str().expect("display"))
            .collect();
        assert_eq!(
            displays,
            [
                "src/lib.rs::outer_alpha::shared_helper",
                "src/lib.rs::outer_beta::shared_helper"
            ],
            "{env}"
        );

        let (code, env) = world.run(bin, &["callers", "outer_alpha::shared_helper"]);
        assert_eq!(code, Some(0), "{bin} callers: {env}");
        assert_eq!(
            env["data"]["hits"][0]["name"], "crate::outer_alpha",
            "{env}"
        );

        let (code, env) = world.run(bin, &["callees", "outer_alpha::shared_helper"]);
        assert_eq!(code, Some(0), "{bin} callees alpha: {env}");
        assert_eq!(env["data"]["hits"][0]["name"], "crate::top_helper", "{env}");

        let (code, env) = world.run(bin, &["callees", "outer_beta::shared_helper"]);
        assert_eq!(code, Some(0), "{bin} callees beta: {env}");
        assert_eq!(env["data"]["hits"][0]["name"], "crate::app::aux", "{env}");

        let (code, env) = world.run(bin, &["callees", "nearby_entries"]);
        assert_eq!(code, Some(0), "{bin} callees nearby_entries: {env}");
        assert_eq!(
            env["data"]["hits"][0]["name"], "crate::app::nearby_entries::is_root",
            "{env}"
        );

        let (code, env) = world.run(bin, &["callers", "aux"]);
        assert_eq!(code, Some(0), "{bin} callers aux: {env}");
        let aux_callers: BTreeSet<&str> = env["data"]["hits"]
            .as_array()
            .expect("hits")
            .iter()
            .map(|h| h["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            aux_callers,
            BTreeSet::from([
                "crate::app::App::run::step",
                "crate::app::nearby_entries::is_root",
                "crate::outer_beta::shared_helper",
            ]),
            "{env}"
        );

        let (code, env) = world.run(bin, &["impact", "top_helper"]);
        assert_eq!(code, Some(0), "{bin} impact top_helper: {env}");
        let impacted: BTreeSet<&str> = env["data"]["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .flat_map(|g| g["hits"].as_array().expect("hits"))
            .map(|h| h["name"].as_str().expect("name"))
            .collect();
        assert!(
            impacted.contains("crate::outer_alpha::shared_helper"),
            "{impacted:?}"
        );
        assert!(impacted.contains("crate::outer_alpha"), "{impacted:?}");
        assert!(
            impacted.contains("crate::layered::mid::leaf"),
            "{impacted:?}"
        );
        assert!(impacted.contains("crate::layered::mid"), "{impacted:?}");
        assert!(impacted.contains("crate::layered"), "{impacted:?}");

        let (code, env) = world.run(bin, &["path", "layered", "top_helper"]);
        assert_eq!(code, Some(0), "{bin} path layered -> top_helper: {env}");
        assert_eq!(env["data"]["found"], true, "{env}");
        let hop_keys: Vec<&str> = env["data"]["hops"]
            .as_array()
            .expect("hops")
            .iter()
            .map(|n| n["key"].as_str().expect("key"))
            .collect();
        assert_eq!(
            hop_keys,
            [
                "src/lib.rs#layered::mid#function#0",
                "src/lib.rs#layered::mid::leaf#function#0",
                "src/lib.rs#top_helper#function#0",
            ],
            "{env}"
        );
    }
}
