//! 10,000 seeded generated maps (SPEC-w7 G2).
//!
//! Each case builds a random graph with long and non-ASCII signatures, a
//! random focus, detail and stored rank, and renders it at a budget from 1
//! to 20,000 and at a larger one. Every map must cost at most its budget,
//! be ASCII, render byte for byte the same twice, and its body must be a
//! prefix of the larger budget's body, so a larger budget never drops a
//! line. Case `i` draws from its own generator, seeded from the seed and
//! `i`, so the cases split into [`SHARDS`] tests that run in parallel.
//! `PHANTOM_MAP_SEED` and `PHANTOM_MAP_CASES` rerun with another seed or
//! count.
//!
//! SPEC-w8 G2: about half the cases also get a co-change scan, a random
//! HEAD and random partner sets with lifts around the `also-changes:`
//! cut, drawn from a second generator so every case without a scan is
//! the W7 case unchanged.

use phantom_fractal_analyze::{
    Detail, EdgeKind, Map, MapError, MapRequest, MapSource, RankEdge, RankNode, RankSource,
    packing_budget, render,
};
use std::collections::BTreeMap;

use phantom_fractal_store::{CochangeGroup, CochangePartner, MapFile, MapRow, StoreError};

const SEED: u64 = 0x5eed_0007_0000_0002;
const CASES: u64 = 10_000;
const MAX_BUDGET: u64 = 20_000;
const SHARDS: u64 = 8;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `low..=high`.
    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.next() % (high - low + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.range(1, 100) <= percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.range(0, items.len() as u64 - 1) as usize]
    }
}

/// Words for names, paths and text, some of them not ASCII.
const WORDS: [&str; 14] = [
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
];

/// From `low` to `high` random words, space-separated.
fn text(rng: &mut Rng, low: u64, high: u64) -> String {
    let count = rng.range(low, high);
    (0..count)
        .map(|_| rng.pick(&WORDS))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A generated index.
struct Case {
    files: Vec<MapFile>,
    rows: Vec<MapRow>,
    nodes: Vec<RankNode>,
    edges: Vec<RankEdge>,
    imports: Vec<(i64, i64)>,
    graph_epoch: u64,
    rank_epoch: u64,
    stored: Vec<(String, i64, f64)>,
    /// The scan HEAD, or none without a scan.
    head: Option<String>,
    /// Each path's partners in the store's order.
    partners: BTreeMap<String, Vec<CochangePartner>>,
}

impl RankSource for Case {
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
}

impl MapSource for Case {
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
        let mut seen = std::collections::BTreeSet::new();
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

/// Lifts around the `also-changes:` cut of 2.
const LIFTS: [f64; 6] = [1.0, 1.5, 2.0, 2.0000001, 3.5, 40.0];

/// Gives about half the cases a scan: a HEAD of 40 hex digits and up to 6
/// partners per file, some of them gone from the files table, sorted as
/// the store sorts them.
fn add_scan(rng: &mut Rng, case: &mut Case) {
    if !rng.chance(50) {
        return;
    }
    case.head = Some(
        (0..40)
            .map(|_| char::from(b"0123456789abcdef"[rng.range(0, 15) as usize]))
            .collect(),
    );
    let paths: Vec<String> = case.files.iter().map(|file| file.path.clone()).collect();
    for path in &paths {
        let mut partners: Vec<CochangePartner> = (0..rng.range(0, 6))
            .map(|_| CochangePartner {
                path: (paths.len() > 1 && !rng.chance(10))
                    .then(|| paths[rng.range(0, paths.len() as u64 - 1) as usize].clone())
                    .filter(|other| other != path),
                together: rng.range(1, 9) as f64 / 2.0,
                n_of: 9.0,
                n_other: 9.0,
                lift: LIFTS[rng.range(0, LIFTS.len() as u64 - 1) as usize],
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
        case.partners.insert(path.clone(), partners);
    }
}

const KINDS: [&str; 5] = ["function", "struct", "enum", "trait", "method"];

fn generate(rng: &mut Rng) -> Case {
    let mut case = Case {
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
    let mut next_id = rng.range(1, 50) as i64;
    let file_count = rng.range(0, 12);
    for position in 0..file_count {
        let id = next_id;
        next_id += rng.range(1, 3) as i64;
        let path = format!("src/{}/{position}.rs", text(rng, 1, 4));
        case.files.push(MapFile { id, path });
        for _ in 0..rng.range(0, 15) {
            let symbol = next_id;
            next_id += 1;
            let kind = rng.pick(&KINDS);
            let name = rng.pick(&WORDS).to_owned();
            let is_test = rng.chance(10);
            case.nodes.push(RankNode {
                id: symbol,
                file_id: id,
                is_test,
                boosted: rng.chance(30),
            });
            if is_test {
                continue;
            }
            let signature = match rng.range(0, 9) {
                0 => None,
                1 => Some(String::new()),
                2 => Some(format!("pub fn {name}({})", text(rng, 20, 60))),
                _ => Some(format!("{kind} {name}{}", text(rng, 0, 6))),
            };
            case.rows.push(MapRow {
                id: symbol,
                file_id: id,
                kind: kind.to_owned(),
                visibility: "public".to_owned(),
                name,
                owner: (kind == "method").then(|| rng.pick(&WORDS).to_owned()),
                signature,
                doc_summary: rng.chance(50).then(|| text(rng, 0, 30)),
                start_line: rng.chance(97).then(|| rng.range(1, 2_000) as i64),
            });
        }
    }
    let ids: Vec<i64> = case.nodes.iter().map(|node| node.id).collect();
    if !ids.is_empty() {
        for _ in 0..rng.range(0, 3 * ids.len() as u64) {
            let source = ids[rng.range(0, ids.len() as u64 - 1) as usize];
            let target = ids[rng.range(0, ids.len() as u64 - 1) as usize];
            case.edges.push(RankEdge {
                kind: if rng.chance(20) {
                    EdgeKind::Impl
                } else {
                    EdgeKind::Call
                },
                source,
                target,
                confidence: [0.9, 0.95, 1.0][rng.range(0, 2) as usize],
            });
        }
    }
    let files: Vec<i64> = case.files.iter().map(|file| file.id).collect();
    if !files.is_empty() {
        for _ in 0..rng.range(0, 2 * files.len() as u64) {
            let from = files[rng.range(0, files.len() as u64 - 1) as usize];
            let to = files[rng.range(0, files.len() as u64 - 1) as usize];
            case.imports.push((from, to));
        }
    }
    if rng.chance(40) {
        // A stored rank, current or stale, with ties.
        case.rank_epoch = if rng.chance(70) {
            case.graph_epoch
        } else {
            case.graph_epoch + 1
        };
        for id in &ids {
            case.stored
                .push(("symbol".to_owned(), *id, rng.range(0, 4) as f64 / 10.0));
        }
        for id in &files {
            case.stored
                .push(("file".to_owned(), *id, rng.range(0, 4) as f64 / 10.0));
        }
    }
    case
}

fn request(rng: &mut Rng, case: &Case) -> MapRequest {
    let mut focus_files = Vec::new();
    if rng.chance(40) {
        for _ in 0..rng.range(1, 3) {
            match case.files.len() {
                0 => focus_files.push(-1),
                n => focus_files.push(case.files[rng.range(0, n as u64 - 1) as usize].id),
            }
        }
        if rng.chance(20) {
            focus_files.push(1_000_000);
        }
    }
    MapRequest {
        // One in ten budgets is small enough to reach BudgetTooSmall.
        budget: if rng.chance(10) {
            rng.range(1, 100)
        } else {
            rng.range(1, MAX_BUDGET)
        },
        focus_files,
        detail: [Detail::Files, Detail::Symbols, Detail::Full][rng.range(0, 2) as usize],
    }
}

/// `ceil(bytes / 3.5) + 1`, written out again here.
fn estimate(line: &str) -> u64 {
    (line.len() as f64 / 3.5).ceil() as u64 + 1
}

/// The lines between the header and the footer.
fn body(map: &Map) -> Vec<&str> {
    let lines: Vec<&str> = map.text.lines().collect();
    lines[1..lines.len() - 1].to_vec()
}

fn check(case: &Case, request: &MapRequest, number: u64) -> Option<Map> {
    let first = render(case, request);
    let second = render(case, request);
    match (first, second) {
        (Err(MapError::BudgetTooSmall { budget, .. }), Err(MapError::BudgetTooSmall { .. })) => {
            assert_eq!(budget, request.budget, "case {number}");
            None
        }
        (Ok(map), Ok(again)) => {
            assert_eq!(map, again, "case {number}: not byte-identical");
            assert!(
                map.used_tokens <= request.budget,
                "case {number}: over budget"
            );
            assert!(
                map.used_tokens <= packing_budget(request.budget),
                "case {number}: over 95%"
            );
            assert!(map.text.is_ascii(), "case {number}: not ASCII");
            let used: u64 = map.text.lines().map(estimate).sum();
            assert_eq!(map.used_tokens, used, "case {number}: miscounted");
            Some(map)
        }
        (first, second) => panic!("case {number}: {first:?} then {second:?}"),
    }
}

/// Runs every case `i` with `i % SHARDS == shard`.
fn shard(shard: u64) {
    let seed = std::env::var("PHANTOM_MAP_SEED")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(SEED);
    let cases = std::env::var("PHANTOM_MAP_CASES")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(CASES);
    let (mut rendered, mut too_small, mut partial, mut ran) = (0, 0, 0, 0);
    let (mut headed, mut also) = (0, 0);
    for number in (shard..cases).step_by(SHARDS as usize) {
        ran += 1;
        let mut rng = Rng(seed ^ number.wrapping_mul(0xa076_1d64_78bd_642f));
        let mut case = generate(&mut rng);
        let small = request(&mut rng, &case);
        let mut large = small.clone();
        large.budget = rng.range(small.budget, MAX_BUDGET);
        // The scan comes from its own generator, so the draws above are
        // the W7 case's.
        add_scan(
            &mut Rng(seed ^ number.wrapping_mul(0xe703_7ed1_a0b4_28db) ^ 0xc0c4),
            &mut case,
        );
        let Some(map) = check(&case, &small, number) else {
            too_small += 1;
            check(&case, &large, number);
            continue;
        };
        rendered += 1;
        if map.files_omitted + map.symbols_omitted > 0 {
            partial += 1;
        }
        let header = map.text.lines().next().unwrap_or_default();
        assert_eq!(
            header.starts_with("phantom-fractal map | head "),
            case.head.is_some(),
            "case {number}: {header}"
        );
        if case.head.is_some() {
            headed += 1;
        }
        if map.text.contains("  also-changes: ") {
            also += 1;
        }
        let bigger = check(&case, &large, number)
            .unwrap_or_else(|| panic!("case {number}: a larger budget is too small"));
        let (lines, more) = (body(&map), body(&bigger));
        assert!(
            more.starts_with(&lines),
            "case {number}: budget {} lost a line of budget {}",
            large.budget,
            small.budget
        );
    }
    // The generator reaches every outcome.
    assert!(rendered > ran / 2, "{rendered} of {ran}");
    assert!(too_small > 0 && partial > 0, "{too_small} {partial}");
    assert!(headed > 0 && also > 0, "{headed} {also}");
}

#[test]
fn seeded_maps_shard_0() {
    shard(0);
}

#[test]
fn seeded_maps_shard_1() {
    shard(1);
}

#[test]
fn seeded_maps_shard_2() {
    shard(2);
}

#[test]
fn seeded_maps_shard_3() {
    shard(3);
}

#[test]
fn seeded_maps_shard_4() {
    shard(4);
}

#[test]
fn seeded_maps_shard_5() {
    shard(5);
}

#[test]
fn seeded_maps_shard_6() {
    shard(6);
}

#[test]
fn seeded_maps_shard_7() {
    shard(7);
}

#[test]
fn the_shards_cover_every_case_once() {
    let mut seen = vec![0; CASES as usize];
    for shard in 0..SHARDS {
        for number in (shard..CASES).step_by(SHARDS as usize) {
            seen[number as usize] += 1;
        }
    }
    assert!(seen.iter().all(|count| *count == 1));
}

fn row(id: i64, file_id: i64, name: &str, line: i64) -> MapRow {
    MapRow {
        id,
        file_id,
        kind: "function".to_owned(),
        visibility: "public".to_owned(),
        name: name.to_owned(),
        owner: None,
        signature: Some(format!("pub fn {name}()")),
        doc_summary: None,
        start_line: Some(line),
    }
}

fn node(id: i64, file_id: i64) -> RankNode {
    RankNode {
        id,
        file_id,
        is_test: false,
        boosted: false,
    }
}

fn empty_case() -> Case {
    Case {
        files: Vec::new(),
        rows: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        imports: Vec::new(),
        graph_epoch: 1,
        rank_epoch: 0,
        stored: Vec::new(),
        head: None,
        partners: BTreeMap::new(),
    }
}

fn file_lines(map: &Map) -> Vec<&str> {
    body(map)
        .into_iter()
        .filter(|line| !line.starts_with(' '))
        .map(|line| line.split("  [r").next().unwrap_or(line))
        .collect()
}

#[test]
fn focus_files_come_first_even_when_their_import_outranks_them() {
    // `a.rs` and `b.rs` both import `hub.rs`. Personalised to the two of
    // them, `hub.rs` gets 0.459 of the file rank and each focus file 0.270.
    let mut case = empty_case();
    for (id, path) in [(1, "a.rs"), (2, "b.rs"), (3, "hub.rs")] {
        case.files.push(MapFile {
            id,
            path: path.to_owned(),
        });
        case.rows.push(row(10 + id, id, "f", 1));
        case.nodes.push(node(10 + id, id));
    }
    case.imports = vec![(1, 3), (2, 3)];
    let request = MapRequest {
        budget: 1_000,
        focus_files: vec![2, 1],
        detail: Detail::Files,
    };
    let map = render(&case, &request).expect("map");
    assert_eq!(file_lines(&map), ["a.rs", "b.rs", "hub.rs"]);
    let rank = |path: &str| {
        let line = body(&map)
            .into_iter()
            .find(|line| line.starts_with(path))
            .expect("line");
        line.rsplit("[r ")
            .next()
            .expect("rank")
            .trim_end_matches(']')
            .parse::<f64>()
            .expect("rank")
    };
    assert!(rank("hub.rs") > rank("a.rs"));
}

#[test]
fn equal_ranks_break_by_path_then_line() {
    // Ids run against paths and lines; a current stored rank ties them all.
    let mut case = empty_case();
    case.rank_epoch = 1;
    for (id, path) in [(3, "a.rs"), (2, "b.rs"), (1, "c.rs")] {
        case.files.push(MapFile {
            id,
            path: path.to_owned(),
        });
        case.stored.push(("file".to_owned(), id, 0.25));
    }
    for (id, line) in [(30, 1), (20, 5), (10, 9)] {
        case.rows.push(row(id, 3, &format!("at{line}"), line));
        case.nodes.push(node(id, 3));
        case.stored.push(("symbol".to_owned(), id, 0.5));
    }
    let request = MapRequest {
        budget: 1_000,
        focus_files: Vec::new(),
        detail: Detail::Symbols,
    };
    let map = render(&case, &request).expect("map");
    assert_eq!(
        body(&map),
        [
            "a.rs  [r 0.250]",
            "  pub fn at1()  :1",
            "  pub fn at5()  :5",
            "  pub fn at9()  :9",
            "b.rs  [r 0.250]",
            "c.rs  [r 0.250]",
        ]
    );
}
