//! The receiver-hint walk's bound on a long function (SPEC-w11 G0b).

mod common;

use std::fmt::Write as _;
use std::fs;
use std::time::{Duration, Instant};

use common::TempDir;
use phantom_fractal_extract::index;
use phantom_fractal_lang_rust::{MAX_HINT_VISITS, RustLanguage};
use phantom_fractal_store::Store;

/// `let` statements in the generated function, each followed by a call.
const STATEMENTS: usize = 20_000;

/// A package whose `f` takes `x: T` and then runs `STATEMENTS` pairs of
/// `let aN = N;` and `x.m();`. `m` is a method of both `T` and `U`, so a
/// call without a hint gets no edge.
fn long_function() -> String {
    let mut src = String::from(
        "pub struct T;\nimpl T {\n    pub fn m(&self) {}\n}\n\
         pub struct U;\nimpl U {\n    pub fn m(&self) {}\n}\n\
         pub fn f(x: T) {\n",
    );
    for n in 0..STATEMENTS {
        writeln!(src, "    let a{n} = {n};\n    x.m();").expect("write to a string");
    }
    src.push_str("}\n");
    src
}

/// A guard far below the quadratic time and under the default nextest
/// timeout, so the test is deterministic on a shared CI runner. The 10 s
/// debug figure SPEC-w11 G0b names is measured on the build box.
const GUARD: Duration = Duration::from_secs(25);

#[test]
fn a_20000_statement_function_keeps_the_walk_bounded() {
    let repo = TempDir::new("bound");
    fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"bound\"\n",
    )
    .expect("write manifest");
    fs::create_dir(repo.path().join("src")).expect("create src");
    fs::write(repo.path().join("src/lib.rs"), long_function()).expect("write lib");
    let db = TempDir::new("bound-db");
    let mut store = Store::open(&db.path().join("graph.db")).expect("open store");
    let rust = RustLanguage::new().expect("query compiles");

    let start = Instant::now();
    let report = index(repo.path(), &mut store, &rust).expect("index");
    let took = start.elapsed();
    assert!(took < GUARD, "index took {took:?}");

    // The k-th call (from 0) visits its field expression, call, statement
    // and block, the 2k + 1 statements before it, then `f` and its one
    // parameter: 2k + 7 visits. So calls 0 to 2044 keep `type:T`.
    let hinted = (MAX_HINT_VISITS - 7) / 2 + 1;
    assert_eq!(hinted, 2045);
    let graph = report.graph.expect("a first pass rebuilds");
    assert_eq!(graph.edges_by_reason.typed_method, hinted as u64);
    assert_eq!(graph.edges_by_reason.unique_method, 0);
    assert_eq!(graph.counts.method_calls, (STATEMENTS - hinted) as u64);
    assert_eq!(graph.counts.edges, hinted as u64);
}
