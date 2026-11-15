//! The extractor fuzz (SPEC-w13 G0b, D5).
//!
//! Case `n` draws every byte from [`Rng::case`]`(n)`. Its family is `n % 3`:
//!
//! | `n % 3` | Family |
//! |---|---|
//! | 0 | random bytes, 0 to 64 KiB |
//! | 1 | a Rust token soup, up to 64 KiB |
//! | 2 | byte mutations of one real file, from every `fixtures/**/*.rs` and then every `.rs` under `PHANTOM_FUZZ_CORPUS` |
//!
//! Each input is `src/lib.rs` of a one-file lib crate. The indexer refuses
//! a file that is not UTF-8 before any parse, and a random-bytes input does
//! the same: it is counted as `refused_not_utf8` and never parsed (A5). A
//! token soup is always UTF-8, and a mutation goes in lossily decoded. The
//! test extracts the file, builds the resolve input
//! the store builds from that extract, and resolves it. Each input must
//! return without a panic within 250 ms of wall time.
//!
//! The `smoke_*` tests run cases 0 to 1999 in CI, in 200 shards of 10, and
//! assert no panic; they print their times without asserting them (ruling
//! R3). The ignored `full` test asserts both the panic and the time bound. It
//! runs `PHANTOM_FUZZ_CASES` cases from `PHANTOM_FUZZ_FIRST` (default 0)
//! on `PHANTOM_FUZZ_THREADS` threads (default: every core), and prints a
//! path-free summary. `PHANTOM_FUZZ_CORPUS` is a path list; the corpus is
//! read and never written. A mutation case replays over the same corpus,
//! whose file count and digest the summary prints.

mod splitmix;

use std::collections::HashMap;
use std::fs;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use phantom_fractal_lang::{
    CallRef, Extract, Id, Language, Module, ResolveInput, Symbol, SymbolKind, Target, TargetKind,
    UseBinding,
};
use phantom_fractal_lang_rust::{RustLanguage, method_owners, resolve_graph};
use splitmix::{Rng, env_number};

/// The CI smoke count.
const SMOKE: u64 = 2_000;
/// The CI smoke runs as this many tests, so each fits the CI time limit.
const SHARDS: u64 = 200;
/// The most bytes a random-bytes or token-soup input has.
const MAX_BYTES: usize = 64 * 1024;
/// The wall-time bound of one input.
const BOUND: Duration = Duration::from_millis(250);
/// The one file of the crate.
const FILE: &str = "src/lib.rs";

const FAMILIES: [&str; 3] = ["bytes", "soup", "mutation"];

/// The real files the mutation family starts from, in a fixed order.
struct Corpus {
    files: Vec<Vec<u8>>,
    fixture_files: usize,
    /// FNV-1a over each file's relative path and bytes, in order.
    digest: u64,
}

impl Corpus {
    /// Every `fixtures/**/*.rs`, then every `.rs` under each path of
    /// `PHANTOM_FUZZ_CORPUS`, each set sorted by relative path. Symbolic
    /// links and `.git` directories are skipped.
    fn load() -> Self {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let mut roots = vec![fixtures];
        if let Some(list) = std::env::var_os("PHANTOM_FUZZ_CORPUS") {
            roots.extend(std::env::split_paths(&list));
        }
        let mut corpus = Self {
            files: Vec::new(),
            fixture_files: 0,
            digest: 0xcbf2_9ce4_8422_2325,
        };
        for (position, root) in roots.iter().enumerate() {
            assert!(
                root.is_dir(),
                "corpus root {} is not a directory",
                root.display()
            );
            let mut found = Vec::new();
            rust_files(root, root, &mut found);
            found.sort();
            for (relative, path) in found {
                let bytes = fs::read(&path).expect("a corpus file reads");
                corpus.mix(relative.as_bytes());
                corpus.mix(&bytes);
                corpus.files.push(bytes);
            }
            if position == 0 {
                corpus.fixture_files = corpus.files.len();
            }
        }
        assert!(corpus.fixture_files > 0, "no fixture files");
        corpus
    }

    fn mix(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.digest = (self.digest ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
        self.digest = (self.digest ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
}

/// Every `.rs` file under `dir` as `(path relative to root, path)`.
fn rust_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).expect("a corpus directory reads") {
        let entry = entry.expect("a corpus entry reads");
        let kind = entry.file_type().expect("a corpus entry has a type");
        let path = entry.path();
        if kind.is_dir() && entry.file_name() != ".git" {
            rust_files(root, &path, out);
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            out.push((relative.to_string_lossy().into_owned(), path));
        }
    }
}

/// Rust tokens for the soup, a few of them broken.
const TOKENS: [&str; 96] = [
    "fn",
    "pub",
    "pub(crate)",
    "pub(super)",
    "struct",
    "enum",
    "trait",
    "impl",
    "mod",
    "use",
    "let",
    "mut",
    "self",
    "Self",
    "super",
    "crate",
    "match",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "return",
    "async",
    "await",
    "move",
    "where",
    "dyn",
    "const",
    "static",
    "type",
    "unsafe",
    "extern",
    "macro_rules!",
    "as",
    "in",
    "ref",
    "a",
    "b",
    "x",
    "Foo",
    "Bar",
    "new",
    "m",
    "f",
    "r#type",
    "ñame",
    "日本",
    "_",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "<",
    ">",
    "::",
    ":",
    ";",
    ",",
    ".",
    "?",
    "!",
    "#",
    "=",
    "->",
    "=>",
    "&",
    "*",
    "+",
    "-",
    "/",
    "|",
    "'",
    "@",
    "$",
    "..",
    "0",
    "1u8",
    "0x1f",
    "1.5e3",
    "\"s\"",
    "\"\\\"e\\\\",
    "r#\"raw\"#",
    "b'x'",
    "'c'",
    "'\\u{1F980}'",
    "'a",
    "'static",
    "// c\n",
    "/* c */",
    "/*",
    "/// doc\n",
    "//! doc\n",
    "\"",
];

/// Larger pieces, so the soup reaches items, impls, calls and macros.
const FRAGMENTS: [&str; 24] = [
    "fn f() {",
    "pub fn g(x: T) -> Option<U> {",
    "impl T {",
    "impl<A> Tr for a::T<A> {",
    "mod m {",
    "mod n;",
    "#[cfg(test)] mod tests {",
    "#[path = \"x.rs\"] mod p;",
    "#[test] fn t() {",
    "use a::b::{c, d as e, *};",
    "pub use crate::m::*;",
    "struct S { f: Box<T>, g: Vec<u8> }",
    "let x = f()?;",
    "let y = T::new();",
    "x.m(1, 2)",
    "self.f.m()",
    "Self::g()",
    "a::b::c::<u8>()",
    "(make::<T>)().jj();",
    "println!(\"{}\", f(x));",
    "vec![g(), h.k()]",
    "macro_rules! mm { ($e:expr) => { f($e) }; }",
    "#![allow(dead_code)]",
    "}",
];

const OPENERS: [&str; 5] = ["(", "{", "[", "mod m {", "fn f() {"];

fn soup(rng: &mut Rng) -> Vec<u8> {
    let target = rng.index(MAX_BYTES + 1);
    let mut out = String::new();
    while out.len() < target {
        let piece = match rng.below(20) {
            0..=13 => *rng.pick(&TOKENS),
            14..=18 => *rng.pick(&FRAGMENTS),
            _ => {
                let opener = *rng.pick(&OPENERS);
                for _ in 0..rng.range(1, 64) {
                    out.push_str(opener);
                }
                ""
            }
        };
        let gap = *rng.pick(&["", " ", " ", "\n", "\t"]);
        out.push_str(piece);
        out.push_str(gap);
    }
    let mut bytes = out.into_bytes();
    bytes.truncate(target);
    bytes
}

fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    let len = rng.index(MAX_BYTES + 1);
    // Every byte, ASCII only, or characters Rust source is made of.
    let class = rng.below(3);
    (0..len)
        .map(|_| match class {
            0 => rng.byte(),
            1 => rng.byte() & 0x7f,
            _ => *rng.pick(b"abcfnxyzST_019 \n\t(){}[]<>:;,.?!#=&*'\"/-+|@$\\"),
        })
        .collect()
}

fn mutation(rng: &mut Rng, corpus: &Corpus) -> Vec<u8> {
    let mut bytes = rng.pick(&corpus.files).clone();
    for _ in 0..rng.range(1, 16) {
        let at = rng.index(bytes.len() + 1);
        match rng.below(8) {
            0 if !bytes.is_empty() => {
                let at = rng.index(bytes.len());
                bytes[at] = rng.byte();
            }
            1 if !bytes.is_empty() => {
                let at = rng.index(bytes.len());
                bytes[at] ^= 1 << rng.below(8);
            }
            2 => {
                let insert: Vec<u8> = (0..rng.range(1, 8)).map(|_| rng.byte()).collect();
                bytes.splice(at..at, insert);
            }
            3 => {
                let end = (at + rng.index(65)).min(bytes.len());
                bytes.drain(at..end);
            }
            4 => {
                let end = (at + rng.index(257)).min(bytes.len());
                let copy = bytes[at..end].to_vec();
                let to = rng.index(bytes.len() + 1);
                bytes.splice(to..to, copy);
            }
            5 => {
                let token = rng.pick(&TOKENS).as_bytes().to_vec();
                bytes.splice(at..at, token);
            }
            6 => {
                let other = rng.pick(&corpus.files);
                let from = rng.index(other.len() + 1);
                let end = (from + rng.index(1025)).min(other.len());
                bytes.splice(at.., other[from..end].iter().copied());
            }
            _ => bytes.truncate(at),
        }
    }
    bytes
}

/// The input of case `number`.
fn input(number: u64, corpus: &Corpus) -> Vec<u8> {
    let mut rng = Rng::case(number);
    match number % 3 {
        0 => random_bytes(&mut rng),
        1 => soup(&mut rng),
        _ => mutation(&mut rng, corpus),
    }
}

/// The resolve input the store builds for a lib crate whose one file,
/// `src/lib.rs`, gives `extract`: the root module, one module per inline
/// `mod`, every item and method as a symbol in the module of its scope,
/// and every `use` and call in the file's module with its scope.
fn one_file_crate(extract: &Extract) -> ResolveInput {
    let mut input = ResolveInput::default();
    input.targets.push(Target {
        id: 1,
        package: "fuzz".to_owned(),
        name: "fuzz".to_owned(),
        kind: TargetKind::Lib,
        root: FILE.to_owned(),
    });
    input
        .modules
        .push(module(1, None, "crate".to_owned(), String::new()));
    let mut scopes: HashMap<String, Id> = HashMap::from([(String::new(), 1)]);
    for decl in extract.mod_decls.iter().filter(|decl| decl.inline) {
        let Some(&parent) = scopes.get(&decl.scope) else {
            continue;
        };
        let id = Id::try_from(input.modules.len()).expect("ids fit") + 1;
        let parent_path = &input.modules[usize::try_from(parent).expect("ids fit") - 1].path;
        let path = format!("{parent_path}::{}", decl.name);
        let scope = if decl.scope.is_empty() {
            decl.name.clone()
        } else {
            format!("{}::{}", decl.scope, decl.name)
        };
        scopes.entry(scope.clone()).or_insert(id);
        input.modules.push(module(id, Some(parent), path, scope));
    }
    let mut next: Id = 1_000;
    // `(owner, name, start line)` of each symbol, as the store finds a
    // call's enclosing function.
    let mut callers: HashMap<(Option<String>, String, u32), Id> = HashMap::new();
    for (position, item) in extract.items.iter().enumerate() {
        let scope = extract.item_scopes[position].clone();
        callers
            .entry((None, item.name.clone(), item.start_line))
            .or_insert(next);
        input.symbols.push(Symbol {
            id: next,
            kind: SymbolKind::from(item.kind),
            name: item.name.clone(),
            owner: None,
            owner_path: None,
            trait_name: None,
            module: scopes.get(&scope).copied(),
            scope,
            file: FILE.to_owned(),
            signature: Some(item.signature.clone()),
        });
        next += 1;
    }
    for method in &extract.methods {
        callers
            .entry((
                Some(method.owner.clone()),
                method.name.clone(),
                method.start_line,
            ))
            .or_insert(next);
        input.symbols.push(Symbol {
            id: next,
            kind: SymbolKind::Method,
            name: method.name.clone(),
            owner: Some(method.owner.clone()),
            owner_path: Some(method.owner_path.clone()),
            trait_name: method.trait_name.clone(),
            scope: method.scope.clone(),
            module: scopes.get(&method.scope).copied(),
            file: FILE.to_owned(),
            signature: Some(method.signature.clone()),
        });
        next += 1;
    }
    for (position, import) in extract.imports.iter().enumerate() {
        input.uses.push(UseBinding {
            id: next,
            module: Some(1),
            scope: extract.import_scopes[position].clone(),
            file: FILE.to_owned(),
            path: import.path.clone(),
            alias: import.alias.clone(),
            glob: import.glob,
        });
        next += 1;
    }
    for reference in &extract.refs {
        let from = reference.from.as_ref().and_then(|caller| {
            let key = (caller.owner.clone(), caller.name.clone(), caller.start_line);
            callers.get(&key).copied()
        });
        input.refs.push(CallRef {
            id: next,
            from,
            module: Some(1),
            scope: reference.scope.clone(),
            file: FILE.to_owned(),
            kind: reference.kind,
            text: reference.text.clone(),
            receiver: reference.receiver.clone(),
            receiver_type: reference.receiver_type.clone(),
            shadowed: reference.shadowed,
        });
        next += 1;
    }
    input
}

fn module(id: Id, parent: Option<Id>, path: String, scope: String) -> Module {
    Module {
        id,
        target: 1,
        parent,
        path,
        file: FILE.to_owned(),
        scope,
    }
}

/// What one case did.
struct Outcome {
    elapsed: Duration,
    /// Call edges plus links, or none when the extractor refused the text.
    placed: Option<usize>,
}

/// Extracts and resolves `source`, the way the indexer does one file.
fn run(language: &RustLanguage, source: &str) -> Outcome {
    let start = Instant::now();
    let placed = language.extract(source).ok().map(|extract| {
        let input = one_file_crate(&extract);
        let graph = black_box(resolve_graph(&input));
        black_box(method_owners(&input));
        graph.calls.len() + graph.links.len()
    });
    Outcome {
        elapsed: start.elapsed(),
        placed,
    }
}

/// Totals over a run.
#[derive(Default)]
struct Summary {
    cases: u64,
    per_family: [u64; 3],
    input_bytes: u64,
    /// Random-bytes inputs that are not UTF-8, refused before any parse.
    refused_not_utf8: u64,
    refused: u64,
    placed: u64,
    slowest: (Duration, u64),
    /// The slowest input of each family, as `(time, case)`.
    slowest_per_family: [(Duration, u64); 3],
    /// Each case that panicked, as `case <n> (<family>)`.
    panics: Vec<String>,
    /// Each case over [`BOUND`], as `case <n> (<family>): <ms> ms`.
    slow: Vec<String>,
}

impl Summary {
    fn add(&mut self, other: Self) {
        self.cases += other.cases;
        for (total, count) in self.per_family.iter_mut().zip(other.per_family) {
            *total += count;
        }
        self.input_bytes += other.input_bytes;
        self.refused_not_utf8 += other.refused_not_utf8;
        self.refused += other.refused;
        self.placed += other.placed;
        self.slowest = self.slowest.max(other.slowest);
        for (mine, theirs) in self
            .slowest_per_family
            .iter_mut()
            .zip(other.slowest_per_family)
        {
            *mine = (*mine).max(theirs);
        }
        self.panics.extend(other.panics);
        self.slow.extend(other.slow);
    }

    /// Fails with every case in `cases`, each replayable from its number.
    fn assert_none(what: &str, cases: &[String]) {
        assert!(
            cases.is_empty(),
            "{} cases {what}; replay one with PHANTOM_FUZZ_FIRST=<n> PHANTOM_FUZZ_CASES=1:\n{}",
            cases.len(),
            cases.join("\n")
        );
    }

    /// One path-free JSON line.
    fn json(&self, first: u64, corpus: &Corpus) -> String {
        let mut line = format!(
            "{{\"first\":{first},\"cases\":{},\"bytes\":{},\"soup\":{},\"mutation\":{},\
             \"fixture_files\":{},\"corpus_files\":{},\"corpus_digest\":\"{:016x}\",\
             \"input_bytes\":{},\"refused_not_utf8\":{},\"refused\":{},\"placed\":{},\"panics\":{},\
             \"over_250_ms\":{},\"max_ms\":{:.3},\"slowest_case\":{}}}",
            self.cases,
            self.per_family[0],
            self.per_family[1],
            self.per_family[2],
            corpus.fixture_files,
            corpus.files.len() - corpus.fixture_files,
            corpus.digest,
            self.input_bytes,
            self.refused_not_utf8,
            self.refused,
            self.placed,
            self.panics.len(),
            self.slow.len(),
            self.slowest.0.as_secs_f64() * 1_000.0,
            self.slowest.1,
        );
        // Per family: the cases over the bound, the slowest time and its case.
        for (family, (time, case)) in FAMILIES.iter().zip(self.slowest_per_family) {
            let tag = format!("({family})");
            let over = self.slow.iter().filter(|slow| slow.contains(&tag)).count();
            line.pop();
            line.push_str(&format!(
                ",\"over_250_ms_{family}\":{over},\"max_ms_{family}\":{:.3},\"slowest_{family}\":{case}}}",
                time.as_secs_f64() * 1_000.0
            ));
        }
        line
    }
}

/// Runs the cases `first + k * step` below `end`.
fn run_cases(first: u64, end: u64, step: u64, corpus: &Corpus) -> Summary {
    let language = RustLanguage::new().expect("the query compiles");
    let mut summary = Summary::default();
    for number in (first..end).step_by(step as usize) {
        let family = FAMILIES[(number % 3) as usize];
        let bytes = input(number, corpus);
        summary.cases += 1;
        summary.per_family[(number % 3) as usize] += 1;
        summary.input_bytes += bytes.len() as u64;
        let Some(source) = source_of(number, &bytes) else {
            summary.refused_not_utf8 += 1;
            continue;
        };
        match catch_unwind(AssertUnwindSafe(|| run(&language, &source))) {
            Err(_) => summary.panics.push(format!("case {number} ({family})")),
            Ok(outcome) => {
                if outcome.elapsed > BOUND {
                    let ms = outcome.elapsed.as_millis();
                    summary
                        .slow
                        .push(format!("case {number} ({family}): {ms} ms"));
                }
                summary.slowest = summary.slowest.max((outcome.elapsed, number));
                let per_family = &mut summary.slowest_per_family[(number % 3) as usize];
                *per_family = (*per_family).max((outcome.elapsed, number));
                match outcome.placed {
                    None => summary.refused += 1,
                    Some(placed) => summary.placed += placed as u64,
                }
            }
        }
    }
    summary
}

/// The text case `number` feeds the extractor: none for a random-bytes
/// input that is not UTF-8, which the indexer refuses before any parse
/// (A5); a mutation lossily decoded.
fn source_of(number: u64, bytes: &[u8]) -> Option<std::borrow::Cow<'_, str>> {
    if number.is_multiple_of(3) {
        return std::str::from_utf8(bytes)
            .ok()
            .map(std::borrow::Cow::Borrowed);
    }
    Some(String::from_utf8_lossy(bytes))
}

/// One CI shard: no case may panic. Its times are printed, not asserted,
/// because a debug build of the C parser cannot meet a release bound
/// (ruling R3); the full run on the build box asserts them.
fn smoke(shard: u64) {
    let corpus = Corpus::load();
    let summary = run_cases(shard, SMOKE, SHARDS, &corpus);
    println!("{}", summary.json(shard, &corpus));
    Summary::assert_none("panicked", &summary.panics);
    let expected = (shard..SMOKE).step_by(SHARDS as usize).count() as u64;
    assert_eq!(summary.cases, expected);
    assert!(summary.per_family.iter().all(|count| *count > 0));
}

/// One `#[test]` per smoke shard.
macro_rules! smoke_shards {
    ($($name:ident = $shard:literal),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                smoke($shard);
            }
        )*
        /// Every shard the macro wrote, in order.
        const SHARD_NUMBERS: &[u64] = &[$($shard),*];
    };
}

smoke_shards!(
    smoke_000 = 0,
    smoke_001 = 1,
    smoke_002 = 2,
    smoke_003 = 3,
    smoke_004 = 4,
    smoke_005 = 5,
    smoke_006 = 6,
    smoke_007 = 7,
    smoke_008 = 8,
    smoke_009 = 9,
    smoke_010 = 10,
    smoke_011 = 11,
    smoke_012 = 12,
    smoke_013 = 13,
    smoke_014 = 14,
    smoke_015 = 15,
    smoke_016 = 16,
    smoke_017 = 17,
    smoke_018 = 18,
    smoke_019 = 19,
    smoke_020 = 20,
    smoke_021 = 21,
    smoke_022 = 22,
    smoke_023 = 23,
    smoke_024 = 24,
    smoke_025 = 25,
    smoke_026 = 26,
    smoke_027 = 27,
    smoke_028 = 28,
    smoke_029 = 29,
    smoke_030 = 30,
    smoke_031 = 31,
    smoke_032 = 32,
    smoke_033 = 33,
    smoke_034 = 34,
    smoke_035 = 35,
    smoke_036 = 36,
    smoke_037 = 37,
    smoke_038 = 38,
    smoke_039 = 39,
    smoke_040 = 40,
    smoke_041 = 41,
    smoke_042 = 42,
    smoke_043 = 43,
    smoke_044 = 44,
    smoke_045 = 45,
    smoke_046 = 46,
    smoke_047 = 47,
    smoke_048 = 48,
    smoke_049 = 49,
    smoke_050 = 50,
    smoke_051 = 51,
    smoke_052 = 52,
    smoke_053 = 53,
    smoke_054 = 54,
    smoke_055 = 55,
    smoke_056 = 56,
    smoke_057 = 57,
    smoke_058 = 58,
    smoke_059 = 59,
    smoke_060 = 60,
    smoke_061 = 61,
    smoke_062 = 62,
    smoke_063 = 63,
    smoke_064 = 64,
    smoke_065 = 65,
    smoke_066 = 66,
    smoke_067 = 67,
    smoke_068 = 68,
    smoke_069 = 69,
    smoke_070 = 70,
    smoke_071 = 71,
    smoke_072 = 72,
    smoke_073 = 73,
    smoke_074 = 74,
    smoke_075 = 75,
    smoke_076 = 76,
    smoke_077 = 77,
    smoke_078 = 78,
    smoke_079 = 79,
    smoke_080 = 80,
    smoke_081 = 81,
    smoke_082 = 82,
    smoke_083 = 83,
    smoke_084 = 84,
    smoke_085 = 85,
    smoke_086 = 86,
    smoke_087 = 87,
    smoke_088 = 88,
    smoke_089 = 89,
    smoke_090 = 90,
    smoke_091 = 91,
    smoke_092 = 92,
    smoke_093 = 93,
    smoke_094 = 94,
    smoke_095 = 95,
    smoke_096 = 96,
    smoke_097 = 97,
    smoke_098 = 98,
    smoke_099 = 99,
    smoke_100 = 100,
    smoke_101 = 101,
    smoke_102 = 102,
    smoke_103 = 103,
    smoke_104 = 104,
    smoke_105 = 105,
    smoke_106 = 106,
    smoke_107 = 107,
    smoke_108 = 108,
    smoke_109 = 109,
    smoke_110 = 110,
    smoke_111 = 111,
    smoke_112 = 112,
    smoke_113 = 113,
    smoke_114 = 114,
    smoke_115 = 115,
    smoke_116 = 116,
    smoke_117 = 117,
    smoke_118 = 118,
    smoke_119 = 119,
    smoke_120 = 120,
    smoke_121 = 121,
    smoke_122 = 122,
    smoke_123 = 123,
    smoke_124 = 124,
    smoke_125 = 125,
    smoke_126 = 126,
    smoke_127 = 127,
    smoke_128 = 128,
    smoke_129 = 129,
    smoke_130 = 130,
    smoke_131 = 131,
    smoke_132 = 132,
    smoke_133 = 133,
    smoke_134 = 134,
    smoke_135 = 135,
    smoke_136 = 136,
    smoke_137 = 137,
    smoke_138 = 138,
    smoke_139 = 139,
    smoke_140 = 140,
    smoke_141 = 141,
    smoke_142 = 142,
    smoke_143 = 143,
    smoke_144 = 144,
    smoke_145 = 145,
    smoke_146 = 146,
    smoke_147 = 147,
    smoke_148 = 148,
    smoke_149 = 149,
    smoke_150 = 150,
    smoke_151 = 151,
    smoke_152 = 152,
    smoke_153 = 153,
    smoke_154 = 154,
    smoke_155 = 155,
    smoke_156 = 156,
    smoke_157 = 157,
    smoke_158 = 158,
    smoke_159 = 159,
    smoke_160 = 160,
    smoke_161 = 161,
    smoke_162 = 162,
    smoke_163 = 163,
    smoke_164 = 164,
    smoke_165 = 165,
    smoke_166 = 166,
    smoke_167 = 167,
    smoke_168 = 168,
    smoke_169 = 169,
    smoke_170 = 170,
    smoke_171 = 171,
    smoke_172 = 172,
    smoke_173 = 173,
    smoke_174 = 174,
    smoke_175 = 175,
    smoke_176 = 176,
    smoke_177 = 177,
    smoke_178 = 178,
    smoke_179 = 179,
    smoke_180 = 180,
    smoke_181 = 181,
    smoke_182 = 182,
    smoke_183 = 183,
    smoke_184 = 184,
    smoke_185 = 185,
    smoke_186 = 186,
    smoke_187 = 187,
    smoke_188 = 188,
    smoke_189 = 189,
    smoke_190 = 190,
    smoke_191 = 191,
    smoke_192 = 192,
    smoke_193 = 193,
    smoke_194 = 194,
    smoke_195 = 195,
    smoke_196 = 196,
    smoke_197 = 197,
    smoke_198 = 198,
    smoke_199 = 199,
);

#[test]
fn mutated_fixtures_still_place_edges() {
    // The first 60 mutation cases; mutations of the fixtures are cheap.
    let summary = run_cases(2, 2 + 3 * 60, 3, &Corpus::load());
    Summary::assert_none("panicked", &summary.panics);
    assert_eq!(summary.per_family, [0, 0, 60]);
    assert!(summary.placed > 0, "no mutated fixture placed an edge");
}

#[test]
fn the_smoke_shards_cover_every_case_once() {
    assert_eq!(SHARD_NUMBERS, (0..SHARDS).collect::<Vec<_>>());
    let mut seen = vec![0; SMOKE as usize];
    for shard in 0..SHARDS {
        for number in (shard..SMOKE).step_by(SHARDS as usize) {
            seen[number as usize] += 1;
        }
    }
    assert!(seen.iter().all(|count| *count == 1));
}

#[test]
fn a_case_replays_from_its_number() {
    let corpus = Corpus::load();
    for number in [0, 1, 2, 1_999, 499_999] {
        assert_eq!(
            input(number, &corpus),
            input(number, &corpus),
            "case {number}"
        );
    }
    assert_ne!(input(0, &corpus), input(3, &corpus));
}

#[test]
fn the_one_file_crate_resolves_its_own_calls() {
    let source = "mod m { pub fn g() {} }\n\
                  struct S;\n\
                  impl S { fn h(&self) { self.k(); } fn k(&self) {} }\n\
                  fn f() { m::g(); S::h(&S); }\n";
    let extract = RustLanguage::new()
        .expect("the query compiles")
        .extract(source)
        .expect("the source parses");
    let input = one_file_crate(&extract);
    assert_eq!(input.modules.len(), 2);
    assert_eq!(input.modules[1].path, "crate::m");
    let (edges, counts) = phantom_fractal_lang_rust::resolve(&input);
    assert_eq!(counts.edges, 3, "{edges:?}");
}

#[test]
#[ignore = "full run: PHANTOM_FUZZ_CASES cases, on the build box"]
fn full() {
    let cases = env_number("PHANTOM_FUZZ_CASES", SMOKE);
    let first = env_number("PHANTOM_FUZZ_FIRST", 0);
    let cores = std::thread::available_parallelism().map_or(1, usize::from) as u64;
    let threads = env_number("PHANTOM_FUZZ_THREADS", cores).max(1);
    let corpus = Corpus::load();
    let end = first + cases;
    let total = Mutex::new(Summary::default());
    let start = Instant::now();
    std::thread::scope(|scope| {
        for thread in 0..threads {
            let (corpus, total) = (&corpus, &total);
            scope.spawn(move || {
                let summary = run_cases(first + thread, end, threads, corpus);
                total.lock().expect("no worker panics").add(summary);
            });
        }
    });
    let summary = total.into_inner().expect("no worker panics");
    let line = summary.json(first, &corpus);
    let wall = start.elapsed().as_secs_f64();
    println!(
        "{},\"threads\":{threads},\"wall_s\":{wall:.1}}}",
        &line[..line.len() - 1]
    );
    assert_eq!(summary.cases, cases);
    Summary::assert_none("panicked", &summary.panics);
    Summary::assert_none("took over 250 ms", &summary.slow);
}
