//! Contract tests covering G0 Go battery semantics across build tags, go.mod/go.work,
//! GoLanguage extraction, and GoResolver Tiers 1-3.

use std::collections::{BTreeMap, HashMap};

use phantom_fractal_lang::{
    CallRef, Graph, Id, Language, Module, Reason, RefKind, ResolveInput, Resolver, Symbol,
    SymbolKind, Target, TargetKind, UseBinding, Visibility,
};
use phantom_fractal_lang_go::go_mod::{GoSourceFile, build_package_graph};
use phantom_fractal_lang_go::{GoLanguage, GoResolver, parse_linknames};

fn build_input(files: &[GoSourceFile<'_>]) -> ResolveInput {
    let pkg_graph = build_package_graph(files);
    let lang = GoLanguage::new();
    let file_contents: HashMap<&str, &str> = files.iter().map(|f| (f.path, f.content)).collect();

    let mut targets = Vec::new();
    let mut modules = Vec::new();
    let mut mod_ids: HashMap<String, Id> = HashMap::new();

    for (idx, pkg) in pkg_graph.packages.iter().enumerate() {
        let tid = i64::try_from(idx + 1).unwrap();
        let mid = i64::try_from(idx + 1).unwrap();
        targets.push(Target {
            id: tid,
            package: pkg.import_path.clone(),
            name: pkg.package_name.clone(),
            kind: if pkg.is_external_test {
                TargetKind::Test
            } else if pkg.is_main {
                TargetKind::Bin
            } else {
                TargetKind::Lib
            },
            root: pkg.files.first().cloned().unwrap_or_default(),
        });
        modules.push(Module {
            id: mid,
            target: tid,
            parent: None,
            path: pkg.import_path.clone(),
            file: String::new(),
            scope: String::new(),
        });
        mod_ids.insert(pkg.import_path.clone(), mid);
    }

    let mut symbols = Vec::new();
    let mut uses = Vec::new();
    let mut refs = Vec::new();
    let mut next_sym: Id = 1;
    let mut next_ref: Id = 1;

    for pkg in &pkg_graph.packages {
        let mid = mod_ids[&pkg.import_path];
        for path in &pkg.files {
            let src = file_contents[path.as_str()];
            let ext = lang.extract(src).unwrap();
            let mut local_callers: Vec<(Option<String>, String, u32, Id)> = Vec::new();
            for item in &ext.items {
                let id = next_sym;
                next_sym += 1;
                symbols.push(Symbol {
                    id,
                    kind: SymbolKind::from(item.kind),
                    name: item.name.clone(),
                    owner: None,
                    owner_path: None,
                    trait_name: None,
                    scope: String::new(),
                    module: Some(mid),
                    file: path.clone(),
                    signature: Some(item.signature.clone()),
                });
                local_callers.push((None, item.name.clone(), item.start_line, id));
            }
            for method in &ext.methods {
                let id = next_sym;
                next_sym += 1;
                symbols.push(Symbol {
                    id,
                    kind: SymbolKind::Method,
                    name: method.name.clone(),
                    owner: Some(method.owner.clone()),
                    owner_path: Some(method.owner_path.clone()),
                    trait_name: method.trait_name.clone(),
                    scope: String::new(),
                    module: Some(mid),
                    file: path.clone(),
                    signature: Some(method.signature.clone()),
                });
                local_callers.push((
                    Some(method.owner.clone()),
                    method.name.clone(),
                    method.start_line,
                    id,
                ));
            }
            for imp in &ext.imports {
                let id = next_ref;
                next_ref += 1;
                uses.push(UseBinding {
                    id,
                    module: Some(mid),
                    scope: String::new(),
                    file: path.clone(),
                    path: imp.path.clone(),
                    alias: imp.alias.clone(),
                    glob: imp.glob,
                });
            }
            for r in &ext.refs {
                let id = next_ref;
                next_ref += 1;
                let from = r.from.as_ref().and_then(|c| {
                    local_callers
                        .iter()
                        .find(|(o, n, ln, _)| {
                            o.as_deref() == c.owner.as_deref()
                                && n == &c.name
                                && *ln == c.start_line
                        })
                        .map(|(_, _, _, sid)| *sid)
                });
                refs.push(CallRef {
                    id,
                    from,
                    module: Some(mid),
                    scope: String::new(),
                    file: path.clone(),
                    kind: r.kind,
                    text: r.text.clone(),
                    receiver: r.receiver.clone(),
                    receiver_type: r.receiver_type.clone(),
                    shadowed: r.shadowed,
                });
            }
        }
    }

    ResolveInput {
        targets,
        modules,
        symbols,
        uses,
        refs,
        deps: Vec::new(),
    }
}

fn edge_names<'a>(input: &'a ResolveInput, graph: &Graph) -> Vec<(&'a str, &'a str, Reason)> {
    let by_id: HashMap<Id, &'a Symbol> = input.symbols.iter().map(|s| (s.id, s)).collect();
    graph
        .calls
        .iter()
        .map(|e| {
            (
                by_id[&e.source].name.as_str(),
                by_id[&e.target].name.as_str(),
                e.reason,
            )
        })
        .collect()
}

#[test]
fn resolves_battery_tiers_one_to_three_without_cross_file_alias_bleed() {
    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/battery\ngo 1.24\n",
        },
        GoSourceFile {
            path: "nested/go.mod",
            content: "module example.com/nested\n",
        },
        GoSourceFile {
            path: "nested/n.go",
            content: "package nested\nfunc N() int { return 1 }\n",
        },
        GoSourceFile {
            path: "testdata/x/x.go",
            content: "package x\nimport \"example.com/battery/internal/core\"\nfunc T() int { return core.Leaf() }\n",
        },
        GoSourceFile {
            path: "lib/lib.go",
            content: r#"package lib

import "sort"

//go:generate ignored_directive
// Shape computes an area.
// Second doc line.
type Shape interface {
	Area() int
}

type Rect struct{ W, H int }

func (r *Rect) Area() int { return r.W * r.H }

type Sq struct{ N int }

func (s Sq) Area() int { return s.N * s.N }

type Clock interface {
	Tick() int
}

type Base struct{}

func (Base) Hello() int { return 7 }

type Outer struct{ Base }

type Set[T comparable] struct{}

func NewSet[T comparable]() *Set[T] { return &Set[T]{} }
func (s *Set[T]) Add(v T) {}

func UseSet() {
	s := NewSet[int]()
	s.Add(1)
}

func UseOuter(o Outer) int { return o.Hello() }

func Direct(s Sq) int { return s.Area() }
func Total(sh Shape) int { return sh.Area() }

func hooked() int { return 1 }
var Hook = func() { _ = hooked() }

type Cmd struct{ RunE func() error }
func run() error { return nil }
func NewCmd() Cmd { return Cmd{RunE: run} }

func double(v int) int { return v * 2 }
func triple(v int) int { return v * 3 }
func less(i, j int) bool { return i < j }

func Map[K any, V any](xs []K, fn func(K) V) {}

func UseMap(xs []int) {
	Map[int, int](xs, double)
}

func Values(c Clock, xs []int) {
	f := c.Tick
	_ = f()
	g := Clock.Tick
	_ = g(c)
	fns := []func(int) int{double, triple}
	_ = fns[0](1)
	sort.Slice(xs, less)
}
"#,
        },
        GoSourceFile {
            path: "lib/os_linux.go",
            content: "package lib\nfunc openFile() string { return \"linux\" }\n",
        },
        GoSourceFile {
            path: "lib/os_darwin.go",
            content: "package lib\nfunc openFile() string { return \"darwin\" }\n",
        },
        GoSourceFile {
            path: "lib/tagged.go",
            content: "//go:build special\n\npackage lib\nfunc Special() string { return openFile() }\n",
        },
        GoSourceFile {
            path: "lib/lib_test.go",
            content: "package lib\nimport \"testing\"\nfunc TestDirect(t *testing.T) { _ = Direct(Sq{N: 2}); _ = openFile(); t.Fatal(\"x\") }\n",
        },
        GoSourceFile {
            path: "lib/ext_test.go",
            content: "package lib_test\nimport \"example.com/battery/lib\"\nfunc helper() int { return lib.Direct(lib.Sq{N: 1}) }\n",
        },
        GoSourceFile {
            path: "internal/core/core.go",
            content: r#"package core

func Leaf() int { return 1 }
func Mid() int { return Leaf() + 1 }
func worker() {}
func cleanup() {}
func Spawn() {
	go worker()
	defer cleanup()
}
func OnlyXTest() int { return 3 }
func init() { _ = Leaf() }
func init() { _ = Mid() }
"#,
        },
        GoSourceFile {
            path: "internal/core/core_test.go",
            content: "package core_test\nimport \"example.com/battery/internal/core\"\nfunc TestOnly() { _ = core.OnlyXTest() }\n",
        },
        GoSourceFile {
            path: "internal/imp/a.go",
            content: "package imp\nimport x \"example.com/battery/internal/p1\"\nfunc A() int { return x.F() }\n",
        },
        GoSourceFile {
            path: "internal/imp/b.go",
            content: "package imp\nimport x \"example.com/battery/internal/p2\"\nfunc B() int { return x.F() }\n",
        },
        GoSourceFile {
            path: "internal/p1/p1.go",
            content: "package p1\nfunc F() int { return 1 }\n",
        },
        GoSourceFile {
            path: "internal/p2/p2.go",
            content: "package p2\nfunc F() int { return 2 }\n",
        },
        GoSourceFile {
            path: "internal/dot/dot.go",
            content: "package dot\nimport . \"example.com/battery/internal/p1\"\nfunc D() int { return F() }\n",
        },
        GoSourceFile {
            path: "internal/cyc/cyc.go",
            content: "package cyc\ntype A struct{ *B }\ntype B struct{ A }\nfunc Use(a A) int { return a.Missing() }\n",
        },
        GoSourceFile {
            path: "internal/link/link.go",
            content: "package link\nimport _ \"unsafe\"\n//go:linkname hidden example.com/battery/internal/core.Leaf\nfunc hidden() int\nfunc Linked() int { return hidden() }\n",
        },
        GoSourceFile {
            path: "cmd/app/main.go",
            content: "package main\nimport (\"example.com/battery/internal/core\"; \"example.com/battery/lib\")\nfunc Run() int { return core.Mid() + lib.Total(nil) + len(\"x\") }\nfunc main() { _ = Run() }\n",
        },
    ];

    let input = build_input(&files);
    let resolver = GoResolver::new();
    assert_eq!(resolver.name(), "go");
    let (calls, _) = resolver.resolve(&input);
    let graph = resolver.resolve_graph(&input);
    assert_eq!(calls, graph.calls);
    let edges = edge_names(&input, &graph);

    assert!(edges.contains(&("Direct", "Area", Reason::TypedMethod)));
    assert!(edges.contains(&("Total", "Area", Reason::InterfaceMethod)));
    assert!(edges.contains(&("UseSet", "Add", Reason::TypedMethod)));
    assert!(edges.contains(&("UseOuter", "Hello", Reason::EmbeddedMethod)));
    assert!(edges.contains(&("Hook", "hooked", Reason::Local)));
    assert!(edges.contains(&("Spawn", "worker", Reason::Local)));
    assert!(edges.contains(&("Spawn", "cleanup", Reason::Local)));
    assert!(edges.contains(&("UseMap", "Map", Reason::Local)));
    assert!(edges.contains(&("TestDirect", "openFile", Reason::Local)));
    assert!(edges.contains(&("helper", "Direct", Reason::PackageImport)));
    assert!(edges.contains(&("TestOnly", "OnlyXTest", Reason::PackageImport)));
    assert!(edges.contains(&("D", "F", Reason::PackageImport)));
    assert!(edges.contains(&("Linked", "hidden", Reason::Local)));
    assert!(edges.contains(&("Run", "Mid", Reason::PackageImport)));
    assert!(edges.contains(&("Run", "Total", Reason::PackageImport)));
    assert!(edges.contains(&("main", "Run", Reason::Local)));

    let init_leaf_sources: Vec<Id> = graph
        .calls
        .iter()
        .filter(|e| {
            input
                .symbols
                .iter()
                .any(|s| s.id == e.target && s.name == "Leaf")
                && input
                    .symbols
                    .iter()
                    .any(|s| s.id == e.source && s.name == "init")
        })
        .map(|e| e.source)
        .collect();
    let init_mid_sources: Vec<Id> = graph
        .calls
        .iter()
        .filter(|e| {
            input
                .symbols
                .iter()
                .any(|s| s.id == e.target && s.name == "Mid")
                && input
                    .symbols
                    .iter()
                    .any(|s| s.id == e.source && s.name == "init")
        })
        .map(|e| e.source)
        .collect();
    assert_eq!(init_leaf_sources.len(), 1);
    assert_eq!(init_mid_sources.len(), 1);
    assert_ne!(init_leaf_sources[0], init_mid_sources[0]);

    let by_id: HashMap<Id, &Symbol> = input.symbols.iter().map(|s| (s.id, s)).collect();
    let a_targets: Vec<&str> = graph
        .calls
        .iter()
        .filter(|e| by_id[&e.source].name == "A")
        .map(|e| by_id[&e.target].file.as_str())
        .collect();
    let b_targets: Vec<&str> = graph
        .calls
        .iter()
        .filter(|e| by_id[&e.source].name == "B")
        .map(|e| by_id[&e.target].file.as_str())
        .collect();
    assert_eq!(a_targets, vec!["internal/p1/p1.go"]);
    assert_eq!(b_targets, vec!["internal/p2/p2.go"]);

    assert!(!edges.iter().any(|(s, t, _)| *s == "NewCmd" && *t == "run"));
    assert!(!edges.iter().any(|(s, _, _)| *s == "Values"));

    let vrefs: BTreeMap<&str, Vec<&str>> = input
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::ValueRef)
        .fold(BTreeMap::new(), |mut acc, r| {
            let last = r.text.rsplit('.').next().unwrap_or(&r.text);
            acc.entry(last).or_default().push(&r.file);
            acc
        });
    assert!(vrefs.contains_key("run"));
    assert_eq!(vrefs.get("Tick").map(Vec::len), Some(2));
    assert_eq!(vrefs.get("double").map(Vec::len), Some(2));
    assert_eq!(vrefs.get("triple").map(Vec::len), Some(1));
    assert_eq!(vrefs.get("less").map(Vec::len), Some(1));

    assert!(graph.counts.method_calls >= 1);
    assert!(graph.counts.unresolved >= 1);
    assert!(graph.counts.external >= 2);

    let linknames = parse_linknames(
        "//go:linkname\n//go:linkname hidden example.com/battery/internal/core.Leaf\n//go:linkname_nope a b\n",
    );
    assert_eq!(linknames.len(), 1);
    assert_eq!(linknames[0].line, 2);
    assert_eq!(linknames[0].local_name, "hidden");
    assert_eq!(
        linknames[0].target.as_deref(),
        Some("example.com/battery/internal/core.Leaf")
    );
    assert!(
        graph
            .links
            .iter()
            .any(|l| l.kind == phantom_fractal_lang::LinkKind::Import
                && l.reason == phantom_fractal_lang::LinkReason::UseAlias)
    );
    assert!(
        graph
            .links
            .iter()
            .any(|l| l.kind == phantom_fractal_lang::LinkKind::Import
                && l.reason == phantom_fractal_lang::LinkReason::PathExact)
    );
    assert!(
        graph
            .links
            .iter()
            .any(|l| l.kind == phantom_fractal_lang::LinkKind::Embeds)
    );
    let impl_targets: Vec<(&str, &str)> = graph
        .links
        .iter()
        .filter(|l| l.kind == phantom_fractal_lang::LinkKind::Impl)
        .map(|l| {
            (
                by_id[&l.source.id()].name.as_str(),
                by_id[&l.target.id()].name.as_str(),
            )
        })
        .collect();
    assert!(impl_targets.contains(&("Rect", "Shape")));
    assert!(impl_targets.contains(&("Sq", "Shape")));
    assert!(!impl_targets.contains(&("Outer", "Shape")));
}

#[test]
fn handles_multi_return_cross_pkg_generics_diamond_embeds_and_errors() {
    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/ext\n",
        },
        GoSourceFile {
            path: "a/a.go",
            content: r#"package a

type Item[T any] struct{}
func (Item[T]) Show() {}

func NewItem[T any]() (Item[T], error) { return Item[T]{}, nil }
func Void() {}
func Single() Item[int] { return Item[int]{} }
func WithParenReturn() (func(int) int, *Item[int]) { return nil, nil }

type Left struct{}
func (Left) Shared() {}
type Right struct{}
func (Right) Shared() {}
type Both struct {
	Left
	Right
}
type Wrap struct{ *Item[int] }
"#,
        },
        GoSourceFile {
            path: "b/b.go",
            content: r#"package b

import (
	"fmt"
	. "ext_dot/pkg"
	a "example.com/ext/a"
)

func Use(w a.Wrap, b a.Both) {
	it, _ := a.NewItem[int]()
	it.Show()
	p := &a.Left{}
	p.Shared()
	_ = a.WithParenReturn()
	s := a.Single()
	s.Show()
	v := Void
	_ = v
	w.Show()
	b.Shared()
	_ = a.Void()
	_ = Unknown()
	_ = fmt.Println("hi")
	_ = DotExternal()
	var nope a.Both
	_ = nope.NonExistent()
	_ = missingPkg.UnknownCall()
	u.What()
}
"#,
        },
    ];
    let input = build_input(&files);
    let graph = GoResolver::new().resolve_graph(&input);
    let edges = edge_names(&input, &graph);
    assert!(edges.contains(&("Use", "NewItem", Reason::PackageImport)));
    assert!(edges.contains(&("Use", "Show", Reason::TypedMethod)));
    assert!(edges.contains(&("Use", "Show", Reason::EmbeddedMethod)));
    // Ambiguous diamond `Both.Shared` must NOT emit an edge; only `p.Shared()` resolves.
    assert_eq!(edges.iter().filter(|(_, t, _)| *t == "Shared").count(), 1);
    // Unresolved external dot, unknown local, ambiguous diamond, missing method:
    assert!(graph.counts.unresolved >= 3);
    assert!(graph.counts.external >= 2);
}

#[test]
fn detects_go_126_and_127_parse_errors_and_handles_extract_contract() {
    let lang = GoLanguage::new();
    assert_eq!(lang.name(), "go");
    assert_eq!(lang.extensions(), &["go"]);
    assert_eq!(lang.qualifier_separator(), ".");

    let new_go = "package newsyntax\nfunc P() *int { return new(core.Leaf()) }\n";
    let gen_go = "package newsyntax\ntype S struct{}\nfunc (s *S) M[T any](v T) T { _ = core.Mid(); return v }\n";
    assert!(lang.has_parse_errors(new_go));
    assert!(lang.has_parse_errors(gen_go));
    assert!(!lang.has_parse_errors("package p\nfunc Ok() int { return 1 }\n"));

    let complex = r#"package extra

import (
	"fmt"
	v "example.com/mod/v2"
)

/* Doc first sentence.
 * Second sentence. */
type (
	Alias = int
	Reader interface {
		fmt.Stringer
		io.Reader | ~int
		Read() (int, error)
	}
	Box struct {
		inner Reader
	}
	Plain int
)

var (
	G int = 1
	_ = UnknownInit()
)

const (
	A = 1
	_ = 2
)

func pair() (Box, error) { return Box{}, nil }

func exercise(items []int, r Reader, b Box, _ int) {
	_, err := pair()
	_ = err
	var x Box = Box{}
	if y, ok := any(x).(Box); ok {
		y.inner.Read()
	}
	b.inner.Read()
	for i, item := range items {
		_ = i
		_ = item
	}
	items[idxFn()] = 42
	f := func(p Reader) { p.Read() }
	f(r)
	func() { helper() }()
	_ = Box{inner: r}
	v.Call()
	var z Reader
	z.Read()
}

func idxFn() int { return 0 }
func helper() {}
"#;
    let ext = lang.extract(complex).unwrap();
    assert!(
        ext.items
            .iter()
            .any(|i| i.name == "Alias" && i.visibility == Visibility::Public)
    );
    assert!(ext.items.iter().any(|i| i.name == "Reader"));
    assert!(ext.items.iter().any(|i| i.name == "Plain"));
    assert!(ext.items.iter().any(|i| i.name == "A"));
    assert!(ext.items.iter().any(|i| i.name == "G"));
    assert!(!ext.items.iter().any(|i| i.name == "_"));
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "idxFn")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "helper")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "f" && r.shadowed)
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "v.Call")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "UnknownInit" && r.from.is_none())
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::ValueRef && r.text == "inner")
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::ValueRef && r.text == "Box")
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::ValueRef && r.text == "i")
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::ValueRef && r.text == "item")
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Embed && (r.text == "{" || r.text == "}"))
    );
}

#[test]
fn checks_exact_counts_scan_edge_cases_and_dot_imports() {
    let lang = GoLanguage::new();
    let scan_src = r#"package scanedge
import (
	_ "unsafe"
	p "example.com/pkg"
)
type Reader interface { Read() }
type Box struct { inner Reader }
var GlobalVar int
var GlobalReader Reader
func pair() (Box, Reader) { return Box{}, nil }
func pairSingle() Box { return Box{} }
func getBox1() Box { return Box{} }
func getBox2() Box { return Box{} }
func getReader() Reader { return nil }
func shadowMe() {}
func valFn() int { return 1 }
func idxFn() int { return 0 }
func demo(cond bool, xs []int, r Reader, b Box) (int, bool, any) {
	if cond {
		shadowMe := 1
		shadowMe()
	}
	shadowMe()
	a, err := pair()
	a.inner.Read()
	err.Read()
	p1, p2 := getBox1(), getReader()
	_ = p1
	p2.Read()
	s := pairSingle()
	s.Read()
	GlobalReader.Read()
	_ = b.inner
	_ = getBox1().Read()
	_ = getBox2().Tag
	_ = (&Box{}).inner
	_ = (&b).inner.Read()
	for i := range xs {
		_ = i
	}
	xs[idxFn()] = valFn()
	GlobalVar = valFn()
	p := Box{}
	_ = p.NewSet[int]()
	var z Reader
	z.Read()
	_ = func(param Reader) { param(); shadowMe(); param.Read() }
	if cond {
		return iota, true, nil
	}
	return 0, false, nil
}
"#;
    let ext = lang.extract(scan_src).unwrap();
    let shadow_calls: Vec<bool> = ext
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::Call && r.text == "shadowMe")
        .map(|r| r.shadowed)
        .collect();
    assert_eq!(shadow_calls, vec![true, false, false]);
    let param_call = ext
        .refs
        .iter()
        .find(|r| r.kind == RefKind::Call && r.text == "param")
        .unwrap();
    assert!(param_call.shadowed);
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "getBox1")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "getBox2")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "valFn")
    );
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "idxFn")
    );
    assert!(
        !ext.refs
            .iter()
            .any(|r| r.kind == RefKind::Call && r.text == "NewSet")
    );
    assert!(!ext.refs.iter().any(|r| {
        r.kind == RefKind::ValueRef
            && matches!(
                r.last_segment.as_str(),
                "inner" | "xs" | "i" | "nil" | "true" | "false" | "iota" | "GlobalVar"
            )
    }));
    assert!(
        ext.refs
            .iter()
            .any(|r| r.kind == RefKind::ValueRef && r.last_segment == "Tag")
    );
    let method_hints: Vec<(&str, Option<&str>)> = ext
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MethodCall && r.text == "Read")
        .map(|r| {
            (
                r.receiver.as_deref().unwrap_or(""),
                r.receiver_type.as_deref(),
            )
        })
        .collect();
    assert!(method_hints.contains(&("(&b).inner", Some("type:Reader"))));
    assert!(method_hints.contains(&("err", Some("ret?:pair"))));
    assert!(method_hints.contains(&("p2", Some("ret:getReader"))));
    assert!(method_hints.contains(&("s", Some("ret:pairSingle"))));
    assert!(method_hints.contains(&("GlobalReader", Some("type:Reader"))));
    assert!(method_hints.contains(&("z", Some("type:Reader"))));
    assert!(method_hints.contains(&("param", Some("type:Reader"))));

    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/c\n",
        },
        GoSourceFile {
            path: "p1/p1.go",
            content: r#"package p1
type Deep interface { DeepM() }
type MidI interface { Deep }
type TopI interface { MidI }
type L0 struct{}
func (L0) M2() {}
type L1 struct { L0 }
type L2 struct { L1 }
func NotGeneric() {}
func NotGenericRet() L0 { return L0{} }
func Dup() {}
var FnVar = func() {}
"#,
        },
        GoSourceFile {
            path: "p2/p2.go",
            content: "package p2\nfunc Dup() {}\n",
        },
        GoSourceFile {
            path: "caller/caller.go",
            content: r#"package caller
import (
	_ "unsafe"
	. "example.com/c/p1"
	. "example.com/c/p2"
	. "external/dot"
	p1pkg "example.com/c/p1"
)
var _ = NoSrcCall()
func Run(l L2, t TopI) {
	l.M2()
	t.DeepM()
	NotGeneric[int]()
	p1pkg.NotGeneric[int]()
	FnVar[int]()
	bad := p1pkg.NotGenericRet[int]()
	bad.M2()
	Dup()
	p1.NotGeneric()
	_ = len("a")
	_ = unsafe.Sizeof(0)
	_ = ExtDot()
}
"#,
        },
    ];
    let input = build_input(&files);
    let graph = GoResolver::new().resolve_graph(&input);
    let edges = edge_names(&input, &graph);
    assert!(edges.contains(&("Run", "M2", Reason::EmbeddedMethod)));
    assert!(edges.contains(&("Run", "DeepM", Reason::InterfaceMethod)));
    assert_eq!(graph.counts.edges, 2);
    assert_eq!(graph.counts.no_source, 1);
    assert_eq!(graph.counts.external, 2);
    assert_eq!(graph.counts.unresolved, 8);
    assert_eq!(graph.counts.method_calls, 3);

    // Individual single-call external & generic-builtin checks so `0 + 1` vs `0 * 1` diverges on every arm.
    let single_fmt = build_input(&[GoSourceFile {
        path: "a.go",
        content: "package a\nimport \"fmt\"\nfunc F() { fmt.Println() }\n",
    }]);
    assert_eq!(GoResolver::new().resolve(&single_fmt).1.external, 1);

    let single_ext_method = build_input(&[GoSourceFile {
        path: "a_test.go",
        content: "package a\nimport \"testing\"\nfunc TestX(t *testing.T) { t.Fatal() }\n",
    }]);
    assert_eq!(GoResolver::new().resolve(&single_ext_method).1.external, 1);

    let single_builtin = build_input(&[GoSourceFile {
        path: "a.go",
        content: "package a\nfunc F() { _ = len(\"x\") }\n",
    }]);
    assert_eq!(GoResolver::new().resolve(&single_builtin).1.external, 1);

    let generic_builtin = build_input(&[GoSourceFile {
        path: "a.go",
        content: "package a\nfunc F() { _ = len[int]() }\n",
    }]);
    let (_, gb_counts) = GoResolver::new().resolve(&generic_builtin);
    assert_eq!(gb_counts.external, 0);
    assert_eq!(gb_counts.unresolved, 1);
}
