//! The scorer run (W10 G1, D3), ignored by default: it reads the
//! `PHANTOM_SCIP` export and the `PHANTOM_SCORE_ROOT` the PM names,
//! indexes that root into a temporary data dir with the crate's own
//! `index`, scores the two against each other, and prints one
//! deterministic, path-free JSON object. Two runs print the same
//! bytes; the baseline evidence records them.

#[path = "scip/mod.rs"]
mod scip;

use std::path::PathBuf;

use phantom_fractal_extract::index;
use phantom_fractal_lang::{Edge, Reason};
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::{ScoreCallRef, Store};

/// One throwaway data dir, removed with the run.
struct Temp {
    root: PathBuf,
}

impl Temp {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "phantom-fractal-score-{label}-{}-{nanos}",
            std::process::id()
        ));
        Self { root }
    }

    fn db(&self) -> PathBuf {
        self.root.join("repos").join("score").join("graph.db")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The baseline helper: one blake3 of the export, for
/// `evidence/w10/G1-backend.md`. It does what the evidence needs and
/// never runs in CI.
#[test]
#[ignore = "manual: blake3 of PHANTOM_SCIP for the baseline note"]
fn hash() {
    let scip =
        std::env::var("PHANTOM_SCIP").expect("PHANTOM_SCIP: the rust-analyzer export's path");
    let bytes = std::fs::read(&scip).unwrap_or_else(|err| panic!("{}: {err}", scip));
    println!("{} {}", blake3::hash(&bytes).to_hex(), scip);
}

#[test]
#[ignore = "manual: reads PHANTOM_SCIP and PHANTOM_SCORE_ROOT (GOAL-w10 D3)"]
fn score() {
    let scip =
        std::env::var("PHANTOM_SCIP").expect("PHANTOM_SCIP: the rust-analyzer export's path");
    let root = std::env::var("PHANTOM_SCORE_ROOT")
        .expect("PHANTOM_SCORE_ROOT: the tree the export carries");
    let bytes = std::fs::read(&scip).unwrap_or_else(|err| panic!("{}: {err}", scip));
    let parsed = scip::wire::parse_index(&bytes).unwrap_or_else(|err| panic!("{}: {err}", scip));

    let temp = Temp::new("w10");
    let mut store = Store::open(&temp.db()).expect("open score store");
    let root_path = PathBuf::from(&root);
    index(
        &root_path,
        &mut store,
        &RustLanguage::new().expect("grammar"),
    )
    .expect("index the scored root");
    let report = scip::score::score(&parsed, &store).expect("score");

    // Deterministic: two runs over the same inputs must print the same
    // bytes, and nothing in the reply may be a path.
    let again = scip::score::score(&parsed, &store).expect("score again");
    assert_eq!(report, again, "two runs of the same tree differ");
    let no_path = report
        .lines()
        .any(|line| line.contains("/Users/") || line.contains("/home/"));
    assert!(!no_path, "the report carries an absolute path: {report}");
    println!("{report}");
}

/// F1 per band: a hand-built export over an indexed two-file package.
/// The export carries `src/lib.rs` only, so the `local` edge
/// `caller -> one` is covered and corroborated, and the `path-exact`
/// edge `two -> one` from `src/out.rs` is neither. Each band holds both
/// edges, and its covered figure holds the one.
#[test]
fn each_band_carries_its_covered_figure() {
    let temp = Temp::new("bands");
    let repo = temp.root.join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("create src");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"bands\"\n").expect("manifest");
    std::fs::write(
        repo.join("src/lib.rs"),
        "mod out;\npub fn one() {}\npub fn caller() {\n    one();\n}\n",
    )
    .expect("lib");
    std::fs::write(
        repo.join("src/out.rs"),
        "pub fn two() {\n    crate::one();\n}\n",
    )
    .expect("out");
    let mut store = Store::open(&temp.db()).expect("open store");
    index(&repo, &mut store, &RustLanguage::new().expect("grammar")).expect("index");

    let occurrence = |line, first, last, name: &str, is_definition| scip::wire::Occurrence {
        range: scip::wire::Range {
            start_line: line,
            start_character: first,
            end_line: line,
            end_character: last,
        },
        symbol: format!("rust-analyzer cargo bands 0.0.0 crate#{name}()."),
        is_definition,
    };
    let export = scip::wire::ScipIndex {
        documents: vec![scip::wire::Document {
            relative_path: "src/lib.rs".to_owned(),
            occurrences: vec![
                occurrence(1, 7, 10, "one", true),
                occurrence(2, 7, 13, "caller", true),
                occurrence(3, 4, 7, "one", false),
            ],
        }],
    };
    let report = scip::score::score(&export, &store).expect("score");

    let band = "{\"edges\":2,\"corroborated\":1,\"precision\":0.5000,\
                \"covered\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000}}";
    let bands = format!("\"confidence\":{{\"0.5\":{band},\"0.85\":{band},\"all\":{band}}}");
    assert!(report.contains(&bands), "{report}");
    assert!(
        report.contains(
            "\"local\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000,\
             \"covered\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000}}"
        ),
        "{report}"
    );
    assert!(
        report.contains(
            "\"path-exact\":{\"edges\":1,\"corroborated\":0,\"precision\":0.0000,\
             \"covered\":{\"edges\":0,\"corroborated\":0,\"precision\":0.0000}}"
        ),
        "{report}"
    );
}

/// One SCIP occurrence on a `crate#<name>()` symbol of one test
/// package, as the bands test builds them.
fn occurrence(
    package: &str,
    line: i64,
    first: i64,
    last: i64,
    name: &str,
    is_definition: bool,
) -> scip::wire::Occurrence {
    scip::wire::Occurrence {
        range: scip::wire::Range {
            start_line: line,
            start_character: first,
            end_line: line,
            end_character: last,
        },
        symbol: format!("rust-analyzer cargo {package} 0.0.0 crate#{name}()."),
        is_definition,
    }
}

/// The 0-based line of the `skip`th line that carries `needle`.
fn line_of(lines: &[&str], needle: &str, skip: usize) -> i64 {
    let mut found = 0;
    for (index, line) in lines.iter().enumerate() {
        if line.contains(needle) {
            if found == skip {
                return index as i64;
            }
            found += 1;
        }
    }
    panic!("needle not found: {needle}");
}

/// The D4 buckets, each on a hand-built ref set (W12 G1a): the
/// classifier reads every bucket key, the ret and ret? hints
/// included, and the precedence is fixed when one caller holds
/// several same-named refs.
#[test]
fn each_d4_bucket_classifies_its_hand_built_refs() {
    use scip::score::miss_bucket;

    let reference =
        |kind: &str, text: &str, receiver: Option<&str>, hint: Option<&str>| ScoreCallRef {
            kind: kind.to_owned(),
            text: text.to_owned(),
            receiver: receiver.map(|text| text.to_owned()),
            receiver_type: hint.map(|hint| hint.to_owned()),
            placed: false,
        };
    let one_ref = |kind: &str, receiver: Option<&str>, hint: Option<&str>| {
        vec![reference(kind, "f", receiver, hint)]
    };

    assert_eq!(
        miss_bucket(&one_ref("macro_arg", None, None), "f", false),
        "macro_arg path shape"
    );
    assert_eq!(
        miss_bucket(&one_ref("macro_arg", Some("x"), None), "f", false),
        "macro_arg method shape"
    );
    assert_eq!(
        miss_bucket(&one_ref("macro", None, None), "f", false),
        "macro invocation"
    );
    // A macro_arg ref outranks the invocation that carries it.
    let both = vec![
        reference("macro_arg", "f", Some("x"), None),
        reference("macro", "f", None, None),
    ];
    assert_eq!(miss_bucket(&both, "f", false), "macro_arg method shape");
    let both = vec![
        reference("macro_arg", "f", None, None),
        reference("macro", "f", None, None),
    ];
    assert_eq!(miss_bucket(&both, "f", false), "macro_arg path shape");

    assert_eq!(
        miss_bucket(&one_ref("call", None, None), "f", false),
        "unresolved path call"
    );
    assert_eq!(
        miss_bucket(&one_ref("method_call", Some("x"), None), "f", true),
        "trait-method callee"
    );
    for (hint, bucket) in [
        ("type:T", "identifier with hint type"),
        ("ctor:T::f", "identifier with hint ctor"),
        ("ctor?:T::f", "identifier with hint ctor?"),
        ("ret:f", "identifier with hint ret"),
        ("ret?:f", "identifier with hint ret?"),
    ] {
        assert_eq!(
            miss_bucket(&one_ref("method_call", Some("x"), Some(hint)), "f", false),
            bucket,
            "{hint}"
        );
    }
    assert_eq!(
        miss_bucket(
            &one_ref("method_call", Some("x"), Some("bogus:z")),
            "f",
            false
        ),
        "other"
    );
    assert_eq!(
        miss_bucket(&one_ref("method_call", Some("self"), None), "f", false),
        "identifier with no hint"
    );
    assert_eq!(
        miss_bucket(&one_ref("method_call", Some("self.f"), None), "f", false),
        "self.field"
    );
    for receiver in ["make()", "a.b", "self.f.g()"] {
        assert_eq!(
            miss_bucket(&one_ref("method_call", Some(receiver), None), "f", false),
            "call or chained receiver",
            "{receiver}"
        );
    }
    for receiver in ["v[0]", "<T as U>"] {
        assert_eq!(
            miss_bucket(&one_ref("method_call", Some(receiver), None), "f", false),
            "other",
            "{receiver}"
        );
    }
    let empty: Vec<ScoreCallRef> = Vec::new();
    assert_eq!(miss_bucket(&empty, "f", false), "other");

    // The precedence with several same-named refs in one caller: the
    // hint kind, then the unhinted identifier, self.field, the call or
    // chained receiver, and the catch-all.
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
        reference("method_call", "f", Some("n2"), None),
        reference("method_call", "f", Some("x"), Some("type:T")),
    ];
    assert_eq!(miss_bucket(&mixed, "f", false), "identifier with hint type");
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
        reference("method_call", "f", Some("n2"), None),
    ];
    assert_eq!(miss_bucket(&mixed, "f", false), "identifier with no hint");
    let mixed = vec![
        reference("method_call", "f", Some("make()"), None),
        reference("method_call", "f", Some("self.f"), None),
    ];
    assert_eq!(miss_bucket(&mixed, "f", false), "self.field");
}

/// One extracted case per D4 bucket the scan can produce at this
/// head (W12 G1a): a real index pass records every ref shape, the
/// hand-built export carries one truth pair per case, no edge places
/// any of them, and each bucket reads exactly one in-module miss. At
/// this head the scan records no receiver on a macro_arg ref, so the
/// extracted macro-arg miss reads the path shape; the method shape
/// key is the hand-built refs above and arrives with G1b. The W11
/// coarse keys are gone, and the origin line reads zero with no
/// macro_arg edge.
#[test]
fn each_d4_bucket_holds_its_extracted_case() {
    let temp = Temp::new("d4");
    let repo = temp.root.join("repo");
    std::fs::create_dir_all(repo.join("src")).expect("create src");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"d4\"\n").expect("manifest");
    let source = r#"pub struct Amb1;
pub struct Amb2;
pub struct Amb3;
pub struct Holder {
    pub f: Amb1,
}
pub struct S2;
pub fn make() -> Amb1 {
    Amb1
}
pub fn one() {}
pub fn one_macro() {}
impl Amb1 {
    pub fn jj(&self) {}
}
impl Amb2 {
    pub fn jj(&self) {}
}
impl Amb3 {
    pub fn build() -> Self {
        Amb3
    }
    pub fn build_opt() -> Option<Self> {
        Some(Amb3)
    }
}
impl Holder {
    pub fn caller_self_field(&self) {
        self.f.jj();
    }
}
impl S2 {
    pub fn m(&self) {}
}
pub fn caller_call_recv() {
    make().jj();
}
pub fn caller_ctor() {
    let c = Amb3::build();
    c.jj();
}
pub fn caller_ctor_opt() -> Option<()> {
    let c = Amb3::build_opt()?;
    c.jj();
    Some(())
}
pub fn caller_hinted(t: Amb3) {
    t.jj();
}
pub fn caller_macro_invocation() {
    one_macro!();
}
pub fn caller_macro_path() {
    assert_eq!(one(), 0);
}
pub fn caller_nohint(n: i32) {
    let n2 = n;
    n2.jj();
}
pub fn caller_other(v: Vec<i32>) {
    v[0].jj();
}
"#;
    std::fs::write(repo.join("src/lib.rs"), source).expect("lib");
    let mut store = Store::open(&temp.db()).expect("open store");
    index(&repo, &mut store, &RustLanguage::new().expect("grammar")).expect("index");

    let lines: Vec<&str> = source.lines().collect();
    let document = scip::wire::Document {
        relative_path: "src/lib.rs".to_owned(),
        occurrences: vec![
            occurrence(
                "d4",
                line_of(&lines, "pub fn one() {}", 0),
                7,
                10,
                "one",
                true,
            ),
            occurrence(
                "d4",
                line_of(&lines, "pub fn one_macro() {}", 0),
                7,
                16,
                "one_macro",
                true,
            ),
            occurrence(
                "d4",
                line_of(&lines, "pub fn jj(&self) {}", 0),
                12,
                14,
                "Amb1.jj",
                true,
            ),
            occurrence(
                "d4",
                line_of(&lines, "self.f.jj();", 0),
                8,
                18,
                "Amb1.jj",
                false,
            ),
            occurrence(
                "d4",
                line_of(&lines, "make().jj();", 0),
                4,
                13,
                "Amb1.jj",
                false,
            ),
            occurrence("d4", line_of(&lines, "c.jj();", 0), 4, 9, "Amb1.jj", false),
            occurrence("d4", line_of(&lines, "c.jj();", 1), 4, 9, "Amb1.jj", false),
            occurrence("d4", line_of(&lines, "t.jj();", 0), 4, 9, "Amb1.jj", false),
            occurrence(
                "d4",
                line_of(&lines, "one_macro!();", 0),
                4,
                15,
                "one_macro",
                false,
            ),
            occurrence(
                "d4",
                line_of(&lines, "assert_eq!(one(), 0);", 0),
                15,
                19,
                "one",
                false,
            ),
            occurrence("d4", line_of(&lines, "n2.jj();", 0), 4, 9, "Amb1.jj", false),
            occurrence(
                "d4",
                line_of(&lines, "v[0].jj();", 0),
                4,
                11,
                "Amb1.jj",
                false,
            ),
        ],
    };
    let export = scip::wire::ScipIndex {
        documents: vec![document],
    };
    let report = scip::score::score(&export, &store).expect("score");

    let hit = "{\"in_module\":1,\"orphan\":0}";
    let zero = "{\"in_module\":0,\"orphan\":0}";
    for bucket in [
        "macro_arg path shape",
        "macro invocation",
        "identifier with hint type",
        "identifier with hint ctor",
        "identifier with hint ctor?",
        "identifier with no hint",
        "self.field",
        "call or chained receiver",
    ] {
        assert!(
            report.contains(&format!("\"{bucket}\":{hit}")),
            "missing {bucket}: {report}"
        );
    }
    // The method-shape catch-all holds the index receiver, and nothing
    // else lands there.
    assert!(
        report.contains("\"other\":{\"in_module\":1,\"orphan\":0}"),
        "{report}"
    );
    // The W11 coarse buckets are gone: the D4 partition replaced them.
    for gone in [
        "macro argument",
        "hinted but not placed",
        "non-self method call with no hint",
    ] {
        assert!(!report.contains(gone), "coarse key remains: {report}");
    }
    assert!(
        report.contains("\"from_macro_arg\":{\"edges\":0,\"corroborated\":0,\"precision\":0.0000}"),
        "{report}"
    );
    // The NULL-receiver base (R1): the method-shape key reads zero
    // until the scan carries receivers.
    assert!(
        report.contains(&format!("\"macro_arg method shape\":{}", zero)),
        "{report}"
    );
}

/// The origin line (W12 D4): an edge whose ref is a macro_arg ref
/// counts there, corroborates against the truth pair, and reads zero
/// precision without one.
#[test]
fn the_origin_line_counts_macro_arg_edges() {
    for corroborated in [true, false] {
        let temp = Temp::new("origin");
        let repo = temp.root.join("repo");
        std::fs::create_dir_all(repo.join("src")).expect("create src");
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"origin\"\n")
            .expect("manifest");
        std::fs::write(
            repo.join("src/lib.rs"),
            "pub fn one() {}\npub fn caller() {\n    assert_eq!(one(), 0);\n}\n",
        )
        .expect("lib");
        let mut store = Store::open(&temp.db()).expect("open store");
        index(&repo, &mut store, &RustLanguage::new().expect("grammar")).expect("index");

        let macro_arg_refs = store.ref_ids_of_kind("macro_arg").expect("macro_arg refs");
        assert_eq!(macro_arg_refs.len(), 1, "one macro_arg ref");
        let ref_id = *macro_arg_refs.iter().next().expect("ref id");
        let spans = store.symbol_spans().expect("spans");
        let caller = spans
            .iter()
            .find(|span| span.name == "caller")
            .expect("caller span")
            .id;
        let one = spans
            .iter()
            .find(|span| span.name == "one")
            .expect("one span")
            .id;
        store
            .replace_edges(&[Edge {
                ref_id,
                source: caller,
                target: one,
                confidence: 0.9,
                reason: Reason::Local,
            }])
            .expect("replace edges");

        let mut occurrences = vec![occurrence("origin", 0, 7, 10, "one", true)];
        if corroborated {
            occurrences.push(occurrence("origin", 2, 15, 19, "one", false));
        }
        let export = scip::wire::ScipIndex {
            documents: vec![scip::wire::Document {
                relative_path: "src/lib.rs".to_owned(),
                occurrences,
            }],
        };
        let report = scip::score::score(&export, &store).expect("score");
        if corroborated {
            assert!(
                report.contains(
                    "\"from_macro_arg\":{\"edges\":1,\"corroborated\":1,\"precision\":1.0000}"
                ),
                "{report}"
            );
            // The pair is a hit now, so the macro_arg bucket reads zero.
            assert!(
                report.contains("\"macro_arg path shape\":{\"in_module\":0,\"orphan\":0}"),
                "{report}"
            );
        } else {
            assert!(
                report.contains(
                    "\"from_macro_arg\":{\"edges\":1,\"corroborated\":0,\"precision\":0.0000}"
                ),
                "{report}"
            );
        }
    }
}
