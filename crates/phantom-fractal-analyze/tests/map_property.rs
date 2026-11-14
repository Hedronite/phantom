//! The map property run (SPEC-w13 G0b, D5, canon §18.1).
//!
//! Case `n` draws a random graph from [`Rng::case`]`(n)`:
//! - up to 40 files and up to 500 symbols, with long and non-ASCII names;
//! - call and impl edges and file imports;
//! - a stored rank that is current, stale or absent;
//! - a co-change scan or none;
//! - a focus and a detail.
//!
//! It renders the graph at three budgets from 0 to 20 000, in ascending
//! order, each twice, and asserts:
//! - `used_tokens` is at most the budget;
//! - the second render equals the first byte for byte, an error included;
//! - a larger budget never removes a line. Each rendered body is a prefix
//!   of the next larger budget's body, and a budget that renders is never
//!   followed by a larger one that does not.
//!
//! The `smoke_*` tests run cases 0 to 499 in CI. The ignored `full` test
//! runs `PHANTOM_MAP_CASES` cases from `PHANTOM_MAP_FIRST` (default 0)
//! and prints a summary. The W7 run in `map_cases.rs` reads
//! `PHANTOM_MAP_CASES` too, so select this binary when setting it.

mod splitmix;

use std::collections::{BTreeMap, BTreeSet};

use phantom_fractal_analyze::{
    Detail, EdgeKind, Map, MapError, MapRequest, MapSource, RankEdge, RankNode, RankSource, render,
};
use phantom_fractal_store::{CochangeGroup, CochangePartner, MapFile, MapRow, StoreError};
use splitmix::{Rng, env_number};

/// The CI smoke count.
const SMOKE: u64 = 500;
/// The CI smoke runs as this many tests, so each fits the CI time limit.
const SHARDS: u64 = 4;
const MAX_FILES: u64 = 40;
const MAX_SYMBOLS: u64 = 500;
const MAX_BUDGET: u64 = 20_000;

/// A generated index.
struct Graph {
    files: Vec<MapFile>,
    rows: Vec<MapRow>,
    nodes: Vec<RankNode>,
    edges: Vec<RankEdge>,
    imports: Vec<(i64, i64)>,
    graph_epoch: u64,
    rank_epoch: u64,
    stored: Vec<(String, i64, f64)>,
    head: Option<String>,
    /// Each path's partners, in the store's order.
    partners: BTreeMap<String, Vec<CochangePartner>>,
}

impl RankSource for Graph {
    fn graph_epoch(&self) -> Result<u64, StoreError> {
        Ok(self.graph_epoch)
    }
    fn nodes(&self) -> Result<Vec<RankNode>, StoreError> {
        Ok(self.nodes.clone())
    }
    fn edges(&self) -> Result<Vec<RankEdge>, StoreError> {
        Ok(self.edges.clone())
    }
    fn imports(&self) -> Result<Vec<(i64, i64)>, StoreError> {
        Ok(self.imports.clone())
    }
    fn file_paths(&self) -> Result<Vec<(i64, String)>, StoreError> {
        Ok(self
            .files
            .iter()
            .map(|file| (file.id, file.path.clone()))
            .collect())
    }
}

impl MapSource for Graph {
    fn map_files(&self) -> Result<Vec<MapFile>, StoreError> {
        Ok(self.files.clone())
    }
    fn map_rows(&self) -> Result<Vec<MapRow>, StoreError> {
        Ok(self.rows.clone())
    }
    fn rank_epoch(&self) -> Result<u64, StoreError> {
        Ok(self.rank_epoch)
    }
    fn stored_rank(&self) -> Result<Vec<(String, i64, f64)>, StoreError> {
        Ok(self.stored.clone())
    }
    fn scan_head(&self) -> Result<Option<String>, StoreError> {
        Ok(self.head.clone())
    }
    fn partners(
        &self,
        paths: &[String],
        min_lift: f64,
        per_file: u32,
    ) -> Result<Vec<CochangeGroup>, StoreError> {
        let mut seen = BTreeSet::new();
        Ok(paths
            .iter()
            .filter(|path| seen.insert(path.as_str()))
            .map(|path| CochangeGroup {
                path: path.clone(),
                partners: self
                    .partners
                    .get(path)
                    .into_iter()
                    .flatten()
                    .filter(|partner| partner.lift >= min_lift)
                    .take(per_file as usize)
                    .cloned()
                    .collect(),
            })
            .collect())
    }
}

/// Words for names, paths and text, some of them not ASCII.
const WORDS: [&str; 16] = [
    "parse",
    "Store",
    "naïve",
    "graph",
    "x",
    "Größe",
    "日本",
    "edge",
    "a_b",
    "Ω",
    "rank",
    "emoji🦀",
    "tab\there",
    "Vec<u8>",
    "a_rather_long_identifier_that_takes_a_line_of_its_own",
    "",
];

const KINDS: [&str; 5] = ["function", "struct", "enum", "trait", "method"];

/// Lifts around the `also-changes:` cut of 2.
const LIFTS: [f64; 6] = [1.0, 1.5, 2.0, 2.000_000_1, 3.5, 40.0];

/// From `low` to `high` random words, space-separated.
fn text(rng: &mut Rng, low: u64, high: u64) -> String {
    (0..rng.range(low, high))
        .map(|_| *rng.pick(&WORDS))
        .collect::<Vec<_>>()
        .join(" ")
}

fn generate(rng: &mut Rng) -> Graph {
    let mut graph = Graph {
        files: Vec::new(),
        rows: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        imports: Vec::new(),
        graph_epoch: rng.range(1, 5),
        rank_epoch: 0,
        stored: Vec::new(),
        head: None,
        partners: BTreeMap::new(),
    };
    let mut next = rng.range(1, 50) as i64;
    for position in 0..rng.range(0, MAX_FILES) {
        let path = format!("src/{}/{position}.rs", text(rng, 1, 4));
        graph.files.push(MapFile { id: next, path });
        next += rng.range(1, 3) as i64;
    }
    if !graph.files.is_empty() {
        // One graph in ten puts every symbol in one file.
        let crowded = rng.chance(10).then(|| rng.index(graph.files.len()));
        for _ in 0..rng.range(0, MAX_SYMBOLS) {
            let file = crowded.unwrap_or_else(|| rng.index(graph.files.len()));
            let file_id = graph.files[file].id;
            add_symbol(rng, &mut graph, next, file_id);
            next += 1;
        }
    }
    add_edges(rng, &mut graph);
    add_rank(rng, &mut graph);
    add_scan(rng, &mut graph);
    graph
}

fn add_symbol(rng: &mut Rng, graph: &mut Graph, id: i64, file_id: i64) {
    let is_test = rng.chance(10);
    graph.nodes.push(RankNode {
        id,
        file_id,
        is_test,
        boosted: rng.chance(30),
    });
    if is_test {
        return;
    }
    let kind = *rng.pick(&KINDS);
    let name = (*rng.pick(&WORDS)).to_owned();
    let signature = match rng.below(10) {
        0 => None,
        1 => Some(String::new()),
        2 => Some(format!("pub fn {name}({})", text(rng, 20, 60))),
        _ => Some(format!("{kind} {name}{}", text(rng, 0, 6))),
    };
    graph.rows.push(MapRow {
        id,
        file_id,
        kind: kind.to_owned(),
        visibility: (*rng.pick(&["public", "crate", "private"])).to_owned(),
        owner: (kind == "method").then(|| (*rng.pick(&WORDS)).to_owned()),
        name,
        signature,
        doc_summary: rng.chance(50).then(|| text(rng, 0, 30)),
        start_line: rng.chance(97).then(|| rng.range(1, 2_000) as i64),
    });
}

fn add_edges(rng: &mut Rng, graph: &mut Graph) {
    let symbols: Vec<i64> = graph.nodes.iter().map(|node| node.id).collect();
    if !symbols.is_empty() {
        for _ in 0..rng.range(0, 3 * symbols.len() as u64) {
            graph.edges.push(RankEdge {
                kind: if rng.chance(20) {
                    EdgeKind::Impl
                } else {
                    EdgeKind::Call
                },
                source: *rng.pick(&symbols),
                target: *rng.pick(&symbols),
                confidence: *rng.pick(&[0.5, 0.85, 0.9, 0.95, 1.0]),
            });
        }
    }
    let files: Vec<i64> = graph.files.iter().map(|file| file.id).collect();
    if !files.is_empty() {
        for _ in 0..rng.range(0, 2 * files.len() as u64) {
            graph.imports.push((*rng.pick(&files), *rng.pick(&files)));
        }
    }
}

/// A stored rank for two graphs in five, current or stale, with ties.
fn add_rank(rng: &mut Rng, graph: &mut Graph) {
    if !rng.chance(40) {
        return;
    }
    graph.rank_epoch = if rng.chance(70) {
        graph.graph_epoch
    } else {
        graph.graph_epoch + 1
    };
    for node in &graph.nodes {
        let score = rng.range(0, 4) as f64 / 10.0;
        graph.stored.push(("symbol".to_owned(), node.id, score));
    }
    for file in &graph.files {
        let score = rng.range(0, 4) as f64 / 10.0;
        graph.stored.push(("file".to_owned(), file.id, score));
    }
}

/// A scan for half the graphs: a HEAD of 40 hex digits and up to 6
/// partners per file, some of them gone from the files table, sorted as
/// the store sorts them.
fn add_scan(rng: &mut Rng, graph: &mut Graph) {
    if !rng.chance(50) {
        return;
    }
    graph.head = Some(
        (0..40)
            .map(|_| char::from(*rng.pick(b"0123456789abcdef")))
            .collect(),
    );
    let paths: Vec<String> = graph.files.iter().map(|file| file.path.clone()).collect();
    for path in &paths {
        let mut partners: Vec<CochangePartner> = (0..rng.range(0, 6))
            .map(|_| CochangePartner {
                path: (!rng.chance(10))
                    .then(|| rng.pick(&paths).clone())
                    .filter(|other| other != path),
                together: rng.range(1, 9) as f64 / 2.0,
                n_of: 9.0,
                n_other: 9.0,
                lift: *rng.pick(&LIFTS),
                conf_of: 0.5,
                conf_to: 0.5,
                last_time: 0,
            })
            .collect();
        partners.sort_by(|a, b| {
            b.lift
                .total_cmp(&a.lift)
                .then(b.together.total_cmp(&a.together))
                .then(a.path.cmp(&b.path))
        });
        graph.partners.insert(path.clone(), partners);
    }
}

fn budget(rng: &mut Rng) -> u64 {
    match rng.below(20) {
        0 => 0,
        1 | 2 => rng.range(0, 100),
        _ => rng.range(0, MAX_BUDGET),
    }
}

/// The focus, the detail and three ascending budgets of one case.
fn requests(rng: &mut Rng, graph: &Graph) -> Vec<MapRequest> {
    let mut focus_files = Vec::new();
    if rng.chance(40) {
        for _ in 0..rng.range(1, 3) {
            focus_files.push(match graph.files.len() {
                0 => -1,
                len => graph.files[rng.index(len)].id,
            });
        }
        if rng.chance(20) {
            focus_files.push(1_000_000);
        }
    }
    let detail = *rng.pick(&[Detail::Files, Detail::Symbols, Detail::Full]);
    let mut budgets = [budget(rng), budget(rng), budget(rng)];
    budgets.sort_unstable();
    budgets
        .into_iter()
        .map(|budget| MapRequest {
            budget,
            focus_files: focus_files.clone(),
            detail,
        })
        .collect()
}

/// The lines between the header and the footer.
fn body(map: &Map) -> Vec<&str> {
    let lines: Vec<&str> = map.text.lines().collect();
    lines[1..lines.len() - 1].to_vec()
}

/// Renders `request` twice: the map, or none when the budget is too
/// small for the header and the footer.
fn render_twice(graph: &Graph, request: &MapRequest, number: u64) -> Option<Map> {
    match (render(graph, request), render(graph, request)) {
        (Ok(map), Ok(again)) => {
            assert_eq!(map, again, "case {number}: a second render differs");
            assert!(
                map.used_tokens <= request.budget,
                "case {number}: {} tokens over budget {}",
                map.used_tokens,
                request.budget
            );
            Some(map)
        }
        (
            Err(MapError::BudgetTooSmall { budget, needed }),
            Err(MapError::BudgetTooSmall {
                budget: again,
                needed: needed_again,
            }),
        ) => {
            assert_eq!((budget, needed), (again, needed_again), "case {number}");
            assert_eq!(budget, request.budget, "case {number}");
            None
        }
        (first, second) => panic!("case {number}: {first:?} then {second:?}"),
    }
}

/// Totals over a run.
#[derive(Default)]
struct Summary {
    cases: u64,
    renders: u64,
    too_small: u64,
    partial: u64,
    complete: u64,
    with_scan: u64,
    max_symbols: u64,
    max_budget_rendered: u64,
}

/// Checks case `number`.
fn check(number: u64, summary: &mut Summary) {
    let mut rng = Rng::case(number);
    let graph = generate(&mut rng);
    summary.cases += 1;
    summary.max_symbols = summary.max_symbols.max(graph.nodes.len() as u64);
    summary.with_scan += u64::from(graph.head.is_some());
    let mut smaller: Option<(u64, Map)> = None;
    for request in requests(&mut rng, &graph) {
        let map = render_twice(&graph, &request, number);
        summary.renders += 1;
        let Some(map) = map else {
            assert!(
                smaller.is_none(),
                "case {number}: budget {} rendered but {} is too small",
                smaller.as_ref().map_or(0, |(budget, _)| *budget),
                request.budget
            );
            summary.too_small += 1;
            continue;
        };
        if map.files_omitted + map.symbols_omitted > 0 {
            summary.partial += 1;
        } else {
            summary.complete += 1;
        }
        summary.max_budget_rendered = summary.max_budget_rendered.max(request.budget);
        if let Some((budget, before)) = &smaller {
            assert!(
                body(&map).starts_with(&body(before)),
                "case {number}: budget {} removed a line of budget {budget}",
                request.budget
            );
        }
        smaller = Some((request.budget, map));
    }
}

fn smoke(shard: u64) {
    let mut summary = Summary::default();
    for number in (shard..SMOKE).step_by(SHARDS as usize) {
        check(number, &mut summary);
    }
    assert_eq!(summary.cases, SMOKE / SHARDS);
    // The generator reaches every outcome.
    assert!(summary.too_small > 0, "no budget was too small");
    assert!(summary.partial > 0, "no map left anything out");
    assert!(summary.complete > 0, "no map showed everything");
    assert!(summary.with_scan > 0, "no graph had a scan");
}

#[test]
fn smoke_0() {
    smoke(0);
}

#[test]
fn smoke_1() {
    smoke(1);
}

#[test]
fn smoke_2() {
    smoke(2);
}

#[test]
fn smoke_3() {
    smoke(3);
}

#[test]
fn the_smoke_shards_cover_every_case_once() {
    let mut seen = vec![0; SMOKE as usize];
    for shard in 0..SHARDS {
        for number in (shard..SMOKE).step_by(SHARDS as usize) {
            seen[number as usize] += 1;
        }
    }
    assert!(seen.iter().all(|count| *count == 1));
}

#[test]
fn the_generator_reaches_500_symbols_and_both_budget_ends() {
    let (mut most, mut zero, mut top) = (0, false, false);
    for number in 0..SMOKE {
        let mut rng = Rng::case(number);
        let graph = generate(&mut rng);
        most = most.max(graph.nodes.len());
        for request in requests(&mut rng, &graph) {
            zero |= request.budget == 0;
            top |= request.budget > MAX_BUDGET - 1_000;
        }
    }
    assert!(most > 450 && most <= MAX_SYMBOLS as usize, "{most}");
    assert!(zero && top);
}

#[test]
#[ignore = "full run: PHANTOM_MAP_CASES cases, on tower"]
fn full() {
    let cases = env_number("PHANTOM_MAP_CASES", SMOKE);
    let first = env_number("PHANTOM_MAP_FIRST", 0);
    let start = std::time::Instant::now();
    let mut summary = Summary::default();
    for number in first..first + cases {
        check(number, &mut summary);
    }
    println!(
        "{{\"first\":{first},\"cases\":{},\"renders\":{},\"too_small\":{},\"partial\":{},\
         \"complete\":{},\"with_scan\":{},\"max_symbols\":{},\"max_budget_rendered\":{},\
         \"over_budget\":0,\"byte_differences\":0,\"lines_removed\":0,\"wall_s\":{:.1}}}",
        summary.cases,
        summary.renders,
        summary.too_small,
        summary.partial,
        summary.complete,
        summary.with_scan,
        summary.max_symbols,
        summary.max_budget_rendered,
        start.elapsed().as_secs_f64(),
    );
    assert_eq!(summary.cases, cases);
}
