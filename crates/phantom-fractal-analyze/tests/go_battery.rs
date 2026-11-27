//! End-to-end G0 red-first 0-false-exact Go battery (`B01`..`B28b`) through
//! `phantom_fractal_extract::{index, diff}`, `Store`, and `phantom_fractal_analyze::{impact, impact_many}`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use phantom_fractal_analyze::{
    CompletenessReason, DiffState, Impact, ImpactOptions, impact, impact_many,
};
use phantom_fractal_extract::lang::{LinkKind, LinkReason};
use phantom_fractal_extract::{diff, index};
use phantom_fractal_git::Base;
use phantom_fractal_lang_rust::RustLanguage;
use phantom_fractal_store::Store;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "phantom-go-battery-{label}-{}-{nanos}-{}",
            process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(root: &Path, rel: &str, content: &str) {
    let full = root.join(rel);
    fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
    fs::write(full, content).expect("write");
}

fn write_battery_tree(root: &Path, include_newsyntax: bool) {
    let files: &[(&str, &str)] = &[
        ("go.mod", "module example.com/battery\n\ngo 1.26.0\n"),
        (
            "lib/shape.go",
            r#"package lib

import "fmt"

type Shape interface{ Area() int }

type Sq struct{ N int }

func (s Sq) Area() int      { return s.N * s.N }
func (s Sq) String() string { return fmt.Sprint("sq", s.N) }

type Rect struct{ W, H int }

func (r *Rect) Area() int { return r.W * r.H }

var _ Shape = (*Rect)(nil)

func Total(xs []Shape) int {
	t := 0
	for _, x := range xs {
		t += x.Area()
	}
	return t
}

func Direct(s Sq) int { return s.Area() }

func Show(s Sq) { fmt.Println(s) }

type W struct{}

func (W) Write(p []byte) (int, error) { return len(p), nil }

func Emit() { fmt.Fprintln(W{}, "x") }
"#,
        ),
        (
            "lib/values.go",
            r#"package lib

import "sort"

type Cmd struct{ RunE func() error }

func run() error { return nil }

func NewCmd() Cmd { return Cmd{RunE: run} }

type Clock struct{}

func (Clock) Tick() int { return 1 }

func double(x int) int { return x * 2 }
func triple(x int) int { return x * 3 }
func less(i, j int) bool { return i < j }

func Values(i int) int {
	c := Clock{}
	f := c.Tick
	g := Clock.Tick
	fns := []func(int) int{double, triple}
	xs := []int{2, 1}
	sort.Slice(xs, less)
	return f() + g(c) + fns[i](1)
}

func Map[T, U any](xs []T, f func(T) U) []U {
	out := make([]U, 0, len(xs))
	for _, x := range xs {
		out = append(out, f(x))
	}
	return out
}

func UseMap() int { return len(Map[int, int]([]int{1}, double)) }

type Set[T comparable] struct{ m map[T]bool }

func NewSet[T comparable]() *Set[T] { return &Set[T]{m: map[T]bool{}} }
func (s *Set[T]) Add(v T)           { s.m[v] = true }

func UseSet() { s := NewSet[int](); s.Add(1) }

var Hook = func() int { return hooked() }

func hooked() int { return 7 }
"#,
        ),
        (
            "lib/embed.go",
            r#"package lib

type Base struct{}

func (Base) Hello() string { return "hi" }

type Outer struct {
	Base
	*Rect
}

func UseOuter(o Outer) string { return o.Hello() }
"#,
        ),
        (
            "lib/os_linux.go",
            "package lib\n\nfunc openFile() string { return \"linux\" }\n",
        ),
        (
            "lib/os_darwin.go",
            "package lib\n\nfunc openFile() string { return \"darwin\" }\n",
        ),
        (
            "lib/tagged.go",
            "//go:build special\n\npackage lib\n\nfunc Special() string { return openFile() + helperTagged() }\n",
        ),
        (
            "lib/lib_test.go",
            r#"package lib

import "testing"

func helperTagged() string { return "" }

func TestDirect(t *testing.T) {
	if Direct(Sq{N: 2}) != 4 {
		t.Fatal("x")
	}
	_ = openFile()
}
"#,
        ),
        (
            "lib/ext_test.go",
            r#"package lib_test

import (
	"testing"

	"example.com/battery/lib"
)

func helper() int { return lib.Direct(lib.Sq{N: 1}) }

func TestExternal(t *testing.T) {
	if helper() != 1 {
		t.Fatal("x")
	}
}
"#,
        ),
        (
            "internal/core/core.go",
            r#"package core

func Leaf() int { return 1 }

func Mid() int { return Leaf() + 1 }

func Spawn() {
	go worker()
	defer cleanup()
}

func worker()  {}
func cleanup() {}

func OnlyXTest() int { return 3 }

func init()  { _ = Leaf() }
func init()  { _ = Mid() }
"#,
        ),
        (
            "internal/core/core_test.go",
            r#"package core_test

import (
	"testing"

	"example.com/battery/internal/core"
)

func TestOnly(t *testing.T) { _ = core.OnlyXTest() }
"#,
        ),
        (
            "internal/imp/a.go",
            "package imp\n\nimport x \"example.com/battery/internal/p1\"\n\nfunc A() int { return x.F() }\n",
        ),
        (
            "internal/imp/b.go",
            "package imp\n\nimport x \"example.com/battery/internal/p2\"\n\nfunc B() int { return x.F() }\n",
        ),
        (
            "internal/p1/p1.go",
            "package p1\n\nfunc F() int { return 1 }\n",
        ),
        (
            "internal/p2/p2.go",
            "package p2\n\nfunc F() int { return 2 }\n",
        ),
        (
            "internal/dot/dot.go",
            "package dot\n\nimport . \"example.com/battery/internal/p1\"\n\nfunc D() int { return F() }\n",
        ),
        (
            "internal/cyc/cyc.go",
            "package cyc\n\ntype A struct{ *B }\ntype B struct{ A }\n\nfunc Use(a A) int { return a.Missing() }\n",
        ),
        (
            "internal/link/link.go",
            "package link\n\nimport _ \"unsafe\"\n\n//go:linkname hidden example.com/battery/internal/core.Leaf\nfunc hidden() int\n\nfunc Linked() int { return hidden() }\n",
        ),
        (
            "cmd/app/main.go",
            r#"package main

import (
	"example.com/battery/internal/core"
	"example.com/battery/lib"
)

func Run() int { return core.Mid() + lib.Total(nil) }

func main() { _ = Run() }
"#,
        ),
        (
            "testdata/x/x.go",
            "package x\n\nimport \"example.com/battery/internal/core\"\n\nfunc T() int { return core.Leaf() }\n",
        ),
        ("nested/go.mod", "module example.com/nested\n\ngo 1.26\n"),
        (
            "nested/n.go",
            "package nested\n\nfunc N() int { return 1 }\n",
        ),
    ];
    for (rel, body) in files {
        write_file(root, rel, body);
    }
    if include_newsyntax {
        write_file(
            root,
            "internal/newsyntax/new.go",
            "package newsyntax\n\nimport \"example.com/battery/internal/core\"\n\nfunc P() *int { return new(core.Leaf()) }\n",
        );
        write_file(
            root,
            "internal/newsyntax/gen.go",
            "package newsyntax\n\nimport \"example.com/battery/internal/core\"\n\ntype S struct{}\n\nfunc (s *S) M[T any](v T) T { _ = core.Mid(); return v }\n",
        );
    }
}

fn find_one(store: &Store, query: &str) -> i64 {
    let m = store.symbols_matching(query).expect("symbols_matching");
    assert_eq!(m.len(), 1, "query `{query}` -> {m:?}");
    m[0].id
}

fn sym_impact(store: &Store, query: &str) -> Impact {
    impact(store, find_one(store, query), &ImpactOptions::default()).expect("impact")
}

fn has_reason(ans: &Impact, r: CompletenessReason) -> bool {
    ans.reasons.iter().any(|x| x.reason == r)
}

fn direct_caller_names(ans: &Impact) -> BTreeSet<String> {
    ans.groups
        .iter()
        .filter(|g| g.depth == 1)
        .flat_map(|g| g.hits.iter().map(|h| h.symbol.name.clone()))
        .chain(
            ans.tests
                .iter()
                .filter(|h| h.depth == 1)
                .map(|h| h.symbol.name.clone()),
        )
        .collect()
}

#[test]
fn b01_to_b27_zero_false_exact_and_exact_controls() {
    let repo = TempDir::new("clean");
    let db_dir = TempDir::new("clean-db");
    write_battery_tree(&repo.0, false);
    let mut store = Store::open(&db_dir.0.join("graph.db")).expect("open");
    let rust = RustLanguage::new().expect("rust");
    let report = index(&repo.0, &mut store, &rust).expect("index");
    assert_eq!(report.not_utf8.len(), 0);

    // B01: Sq.Area
    let b01 = sym_impact(&store, "Sq.Area");
    assert!(!b01.exact);
    assert!(has_reason(&b01, CompletenessReason::InterfaceDispatch));
    assert!(direct_caller_names(&b01).contains("Direct"));

    // B02: Rect.Area + var _ Shape = (*Rect)(nil) link confidence 1.0
    let b02 = sym_impact(&store, "Rect.Area");
    assert!(!b02.exact);
    assert!(has_reason(&b02, CompletenessReason::InterfaceDispatch));
    let rect_id = find_one(&store, "example.com/battery/lib.Rect");
    let shape_id = find_one(&store, "example.com/battery/lib.Shape");
    assert!(store.edges_of_kind("impl").unwrap().iter().any(|e| {
        e.src_id == rect_id
            && e.dst_id == shape_id
            && e.reason == LinkReason::ImplTrait.as_str()
            && (e.confidence - 1.0).abs() < 1e-6
    }));

    // B03, B04: stdlib fmt.Stringer & io.Writer
    for q in ["Sq.String", "W.Write"] {
        let ans = sym_impact(&store, q);
        assert!(!ans.exact, "{q}");
        assert!(
            has_reason(&ans, CompletenessReason::InterfaceDispatch),
            "{q}"
        );
    }

    // B05: Cmd{RunE: run}
    let b05 = sym_impact(&store, "example.com/battery/lib.run");
    assert!(!b05.exact);
    assert!(has_reason(&b05, CompletenessReason::ValueReference));
    assert!(!direct_caller_names(&b05).contains("NewCmd"));

    // B06, B07: Clock.Tick (method value + method expression)
    let b06 = sym_impact(&store, "Clock.Tick");
    assert!(!b06.exact);
    assert!(has_reason(&b06, CompletenessReason::ValueReference));
    assert!(has_reason(&b06, CompletenessReason::InterfaceDispatch));

    // B08: fns[i](1) never emits call edge to double/triple
    for q in [
        "example.com/battery/lib.double",
        "example.com/battery/lib.triple",
    ] {
        let ans = sym_impact(&store, q);
        assert!(!ans.exact, "{q}");
        assert!(has_reason(&ans, CompletenessReason::ValueReference), "{q}");
        assert!(!direct_caller_names(&ans).contains("Values"), "{q}");
    }

    // B09: sort.Slice(xs, less)
    let b09 = sym_impact(&store, "example.com/battery/lib.less");
    assert!(!b09.exact);
    assert!(has_reason(&b09, CompletenessReason::ValueReference));

    // B10: Map[int, int]
    let b10 = sym_impact(&store, "example.com/battery/lib.Map");
    assert!(direct_caller_names(&b10).contains("UseMap"));

    // B11: generic Set.Add
    let b11 = sym_impact(&store, "Set.Add");
    assert!(direct_caller_names(&b11).contains("UseSet"));

    // B12: var Hook = func() { return hooked() }
    let b12 = sym_impact(&store, "example.com/battery/lib.hooked");
    assert!(direct_caller_names(&b12).contains("Hook"));

    // B13: go worker() / defer cleanup() exact:true controls
    for q in [
        "example.com/battery/internal/core.worker",
        "example.com/battery/internal/core.cleanup",
    ] {
        let ans = sym_impact(&store, q);
        assert!(ans.exact, "{q}: {:?}", ans.reasons);
        assert_eq!(
            direct_caller_names(&ans),
            BTreeSet::from(["Spawn".to_owned()])
        );
    }

    // B14: two init() callers, Mid exact:true, did_you_mean excludes init
    let mid = sym_impact(&store, "example.com/battery/internal/core.Mid");
    assert!(mid.exact, "Mid: {:?}", mid.reasons);
    let mid_callers = direct_caller_names(&mid);
    assert!(mid_callers.contains("init"));
    assert!(mid_callers.contains("Run"));
    assert!(
        !store
            .did_you_mean("ini")
            .unwrap()
            .iter()
            .any(|s| s.qual_name == "init")
    );

    // B15: build constraints openFile -> os_linux.go only + build_excluded
    let open_matches = store
        .symbols_matching("example.com/battery/lib.openFile")
        .unwrap();
    assert_eq!(open_matches.len(), 1);
    assert_eq!(open_matches[0].file, "lib/os_linux.go");
    let open_imp = sym_impact(&store, "example.com/battery/lib.openFile");
    assert!(!open_imp.exact);
    assert!(has_reason(&open_imp, CompletenessReason::BuildExcluded));
    let ht_imp = sym_impact(&store, "example.com/battery/lib.helperTagged");
    assert!(!ht_imp.exact);
    assert!(has_reason(&ht_imp, CompletenessReason::BuildExcluded));

    // B16: package foo_test + OnlyXTest exact:true control
    let direct_imp = sym_impact(&store, "example.com/battery/lib.Direct");
    assert!(!direct_imp.exact);
    assert!(has_reason(
        &direct_imp,
        CompletenessReason::ExternalUsersPossible
    ));
    let dc = direct_caller_names(&direct_imp);
    assert!(dc.contains("helper") && dc.contains("TestDirect"), "{dc:?}");
    let only_x = sym_impact(&store, "example.com/battery/internal/core.OnlyXTest");
    assert!(only_x.exact, "OnlyXTest: {:?}", only_x.reasons);
    assert_eq!(
        direct_caller_names(&only_x),
        BTreeSet::from(["TestOnly".to_owned()])
    );

    // B21, B22: per-file alias & dot import precision
    let p1f = sym_impact(&store, "example.com/battery/internal/p1.F");
    assert_eq!(
        direct_caller_names(&p1f),
        BTreeSet::from(["A".to_owned(), "D".to_owned()])
    );
    let p2f = sym_impact(&store, "example.com/battery/internal/p2.F");
    assert_eq!(direct_caller_names(&p2f), BTreeSet::from(["B".to_owned()]));

    // B23: Base.Hello promoted via Outer
    let hello = sym_impact(&store, "Base.Hello");
    assert!(!hello.exact);
    assert!(has_reason(&hello, CompletenessReason::InterfaceDispatch));
    assert!(direct_caller_names(&hello).contains("UseOuter"));

    // B24: embedding cycle A{*B}; B{A}; a.Missing() terminates cleanly
    assert_eq!(store.unresolved_named("Missing").unwrap(), 1);

    // B25: //go:linkname makes core.Leaf exact:false
    let leaf = sym_impact(&store, "example.com/battery/internal/core.Leaf");
    assert!(!leaf.exact);
    assert!(has_reason(&leaf, CompletenessReason::ValueReference));
    assert!(!direct_caller_names(&leaf).contains("hidden"));
    assert!(!direct_caller_names(&leaf).contains("T")); // B27 testdata/x never a caller

    // B26: ExternalUsersPossible controls: Total true, Run false (exact:true), Mid false (exact:true)
    let total = sym_impact(&store, "example.com/battery/lib.Total");
    assert!(!total.exact);
    assert!(has_reason(
        &total,
        CompletenessReason::ExternalUsersPossible
    ));
    let run_fn = sym_impact(&store, "example.com/battery/cmd/app.Run");
    assert!(run_fn.exact, "Run: {:?}", run_fn.reasons);

    // B27: testdata/ and nested/ excluded from store
    assert!(store.symbols_matching("x.T").unwrap().is_empty());
    assert!(store.symbols_matching("nested.N").unwrap().is_empty());
    assert!(
        store
            .edges_of_kind("impl")
            .unwrap()
            .iter()
            .any(|l| l.kind == LinkKind::Impl.as_str())
    );
}

#[test]
fn b28_parse_errors_and_b17_to_b20_diff_impact_hold_zero_false_exact() {
    // B28a & B28b: newsyntax parse errors force exact:false on core.Leaf and core.Mid
    let ns_repo = TempDir::new("newsyntax");
    let ns_db = TempDir::new("newsyntax-db");
    write_battery_tree(&ns_repo.0, true);
    let mut store = Store::open(&ns_db.0.join("graph.db")).expect("open");
    let rust = RustLanguage::new().expect("rust");
    index(&ns_repo.0, &mut store, &rust).expect("index");
    assert!(store.source_skipped().unwrap().unwrap_or(0) >= 2);
    for q in [
        "example.com/battery/internal/core.Leaf",
        "example.com/battery/internal/core.Mid",
    ] {
        let imp = sym_impact(&store, q);
        assert!(!imp.exact);
        assert!(has_reason(&imp, CompletenessReason::ParseSkippedFiles));
    }

    // B17, B18, B19, B20 & R1''': real git worktree diff-impact
    let grepo = TempDir::new("git-diff");
    let gdb = TempDir::new("git-diff-db");
    write_battery_tree(&grepo.0, false);
    run_git(&grepo.0, &["init", "-q", "-b", "main"]);
    run_git(&grepo.0, &["config", "user.email", "test@example.invalid"]);
    run_git(&grepo.0, &["config", "user.name", "Test"]);
    run_git(&grepo.0, &["add", "."]);
    run_git(&grepo.0, &["commit", "-q", "-m", "base"]);

    let mut store = Store::open(&gdb.0.join("graph.db")).expect("open");
    index(&grepo.0, &mut store, &rust).expect("index");

    // B17: body edit of core.Leaf
    let core_orig = fs::read_to_string(grepo.0.join("internal/core/core.go")).unwrap();
    write_file(
        &grepo.0,
        "internal/core/core.go",
        &core_orig.replace(
            "func Leaf() int { return 1 }",
            "func Leaf() int { return 2 }",
        ),
    );
    let d17 = diff(&grepo.0, &Base::Head, &rust).unwrap().unwrap();
    assert!(d17.changed.iter().any(|c| c.name == "Leaf"));
    let src_ids: Vec<i64> = d17
        .changed
        .iter()
        .filter_map(|c| store.symbol_by_stable_key(&c.key).unwrap())
        .collect();
    let imp17 = impact_many(
        &store,
        &src_ids,
        &ImpactOptions::default(),
        &DiffState::default(),
    )
    .unwrap();
    let hit_names: BTreeSet<String> = imp17
        .groups
        .iter()
        .flat_map(|g| g.hits.iter().map(|h| h.symbol.name.clone()))
        .collect();
    assert!(hit_names.contains("Mid") && hit_names.contains("Run") && hit_names.contains("init"));
    write_file(&grepo.0, "internal/core/core.go", &core_orig);

    // B18: go.mod edit
    let mod_orig = fs::read_to_string(grepo.0.join("go.mod")).unwrap();
    write_file(&grepo.0, "go.mod", &format!("{mod_orig}\n// edit\n"));
    let d18 = diff(&grepo.0, &Base::Head, &rust).unwrap().unwrap();
    assert!(d18.manifest_changed);
    let imp18 = impact_many(
        &store,
        &[],
        &ImpactOptions::default(),
        &DiffState {
            manifest_changed: d18.manifest_changed,
            ..DiffState::default()
        },
    )
    .unwrap();
    assert!(!imp18.exact);
    assert!(
        imp18
            .reasons
            .iter()
            .any(|r| r.reason == CompletenessReason::ManifestChanged)
    );
    write_file(&grepo.0, "go.mod", &mod_orig);

    // B19 & B20: build-excluded tagged.go, testdata/x/x.go, and nested/n.go edits -> exact:false
    for (rel, orig_fn, replacement) in [
        ("lib/tagged.go", "openFile() + helperTagged()", "openFile()"),
        (
            "testdata/x/x.go",
            "return core.Leaf()",
            "return core.Leaf() + 1",
        ),
        ("nested/n.go", "return 1", "return 2"),
    ] {
        let orig = fs::read_to_string(grepo.0.join(rel)).unwrap();
        write_file(&grepo.0, rel, &orig.replace(orig_fn, replacement));
        let d = diff(&grepo.0, &Base::Head, &rust).unwrap().unwrap();
        assert!(!d.changed.is_empty() || !d.files_only.is_empty(), "{rel}");
        let unindexed = d
            .changed
            .iter()
            .filter(|c| store.symbol_by_stable_key(&c.key).unwrap().is_none())
            .count() as u64;
        let imp = impact_many(
            &store,
            &[],
            &ImpactOptions::default(),
            &DiffState {
                unindexed_changes: unindexed.max(d.outside_span_changes as u64),
                ..DiffState::default()
            },
        )
        .unwrap();
        assert!(!imp.exact, "{rel}");
        assert!(
            imp.reasons
                .iter()
                .any(|r| r.reason == CompletenessReason::UnindexedChanges)
        );
        write_file(&grepo.0, rel, &orig);
    }

    // R1''': Go import rebind alongside same-file body edit sets outside_span_changes > 0
    write_file(
        &grepo.0,
        "internal/imp/a.go",
        "package imp\n\nimport x \"example.com/battery/internal/p2\"\n\nfunc A() int { return x.F() + 1 }\n",
    );
    let dr1 = diff(&grepo.0, &Base::Head, &rust).unwrap().unwrap();
    assert!(dr1.outside_span_changes > 0);
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
