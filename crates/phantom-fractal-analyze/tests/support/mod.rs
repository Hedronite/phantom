//! A constructed call graph behind [`ImpactSource`].
// Each test binary compiles this module and uses a different part of it.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use phantom_fractal_analyze::{Impact, ImpactOptions, ImpactSource, impact};
use phantom_fractal_store::{CallHit, StoreError, SymbolFacts};

pub struct Graph {
    symbols: BTreeMap<i64, SymbolFacts>,
    callers: HashMap<i64, Vec<CallHit>>,
    pub unresolved: HashMap<String, i64>,
    pub macro_args: HashMap<String, i64>,
    pub rs_skipped: Option<u64>,
}

impl Graph {
    /// An empty graph whose index pass skipped nothing.
    pub fn new() -> Self {
        Self {
            symbols: BTreeMap::new(),
            callers: HashMap::new(),
            unresolved: HashMap::new(),
            macro_args: HashMap::new(),
            rs_skipped: Some(0),
        }
    }

    /// A private function `name` in package `p`'s lib, in `src/lib.rs` at
    /// line `id`.
    pub fn function(&mut self, id: i64, name: &str) -> i64 {
        self.symbols.insert(
            id,
            SymbolFacts {
                id,
                name: name.to_owned(),
                kind: "function".to_owned(),
                file: "src/lib.rs".to_owned(),
                line: Some(id),
                visibility: "private".to_owned(),
                trait_name: None,
                is_test: false,
                package: Some("p".to_owned()),
                target_kind: Some("lib".to_owned()),
            },
        );
        id
    }

    /// Like [`Graph::function`], marked as a test.
    pub fn test(&mut self, id: i64, name: &str) -> i64 {
        self.function(id, name);
        self.facts_mut(id).is_test = true;
        id
    }

    pub fn facts_mut(&mut self, id: i64) -> &mut SymbolFacts {
        self.symbols.get_mut(&id).expect("symbol")
    }

    /// An edge `caller -> callee` with `confidence`, reason `local`, at
    /// line `caller * 1000 + callee` of the caller's file.
    pub fn call(&mut self, caller: i64, callee: i64, confidence: f64) {
        self.call_with(caller, callee, confidence, "local");
    }

    pub fn call_with(&mut self, caller: i64, callee: i64, confidence: f64, reason: &str) {
        let file = self.symbols[&caller].file.clone();
        self.callers.entry(callee).or_default().push(CallHit {
            symbol: caller,
            file,
            line: Some(caller * 1000 + callee),
            confidence,
            reason: reason.to_owned(),
        });
    }

    pub fn impact(&self, target: i64) -> Impact {
        self.impact_with(target, &ImpactOptions::default())
    }

    pub fn impact_with(&self, target: i64, options: &ImpactOptions) -> Impact {
        impact(self, target, options).expect("impact")
    }
}

impl ImpactSource for Graph {
    fn callers(&self, id: i64) -> Result<Vec<CallHit>, StoreError> {
        Ok(self.callers.get(&id).cloned().unwrap_or_default())
    }

    fn facts(&self, id: i64) -> Result<Option<SymbolFacts>, StoreError> {
        Ok(self.symbols.get(&id).cloned())
    }

    fn unresolved_named(&self, name: &str) -> Result<i64, StoreError> {
        Ok(self.unresolved.get(name).copied().unwrap_or(0))
    }

    fn macro_args_named(&self, name: &str) -> Result<i64, StoreError> {
        Ok(self.macro_args.get(name).copied().unwrap_or(0))
    }

    fn rs_skipped(&self) -> Result<Option<u64>, StoreError> {
        Ok(self.rs_skipped)
    }
}

/// `(depth, name, confidence)` for every reported non-test hit.
pub fn hits(impact: &Impact) -> Vec<(u8, String, f64)> {
    impact
        .groups
        .iter()
        .flat_map(|group| {
            group
                .hits
                .iter()
                .map(|hit| (hit.depth, hit.symbol.name.clone(), hit.confidence))
        })
        .collect()
}

/// `(depth, name)` for every reported non-test hit.
pub fn names(impact: &Impact) -> Vec<(u8, &str)> {
    impact
        .groups
        .iter()
        .flat_map(|group| {
            group
                .hits
                .iter()
                .map(|hit| (hit.depth, hit.symbol.name.as_str()))
        })
        .collect()
}

pub fn options(max_depth: u8, min_confidence: f64, include_tests: bool) -> ImpactOptions {
    ImpactOptions {
        max_depth,
        min_confidence,
        include_tests,
    }
}

/// A target `t` (id 1) with `direct` callers at depth 1 and `second`
/// more callers of the first caller at depth 2.
pub fn star(direct: i64, second: i64) -> Graph {
    let mut g = Graph::new();
    g.function(1, "t");
    for n in 0..direct {
        let id = 100 + n;
        g.function(id, &format!("d{n:03}"));
        g.call(id, 1, 1.0);
    }
    for n in 0..second {
        let id = 10_000 + n;
        g.function(id, &format!("s{n:03}"));
        g.call(id, 100, 1.0);
    }
    g
}
