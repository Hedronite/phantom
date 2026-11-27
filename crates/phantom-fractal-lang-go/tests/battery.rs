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
    assert!(edges.contains(&("Run", "M2", Reason::UniqueMethod)));
    assert_eq!(graph.counts.edges, 3);
    assert_eq!(graph.counts.no_source, 1);
    assert_eq!(graph.counts.external, 2);
    assert_eq!(graph.counts.unresolved, 7);
    assert_eq!(graph.counts.method_calls, 2);

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

#[test]
fn resolves_g3b_embedding_tiers_assertions_unique_method_linkname_and_inference() {
    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/g3b
go 1.26.0
",
        },
        GoSourceFile {
            path: "iface/iface.go",
            content: r#"package iface

type Shape interface {
	Area() int
}

type Empty interface{}

type Printer interface {
	Print()
}

type Base struct{}
func (Base) Hello() {}
func (Base) DepthWin() {}

type Left struct{}
func (Left) Clash() {}
type Right struct{}
func (Right) Clash() {}
type Both struct {
	Left
	Right
}

type Deep struct{}
func (Deep) DepthWin() {}
func (Deep) TooDeep() {}
type MidWrap struct{ Deep }
type ShallowWin struct {
	Base
	MidWrap
}

type Outer struct {
	Base
}

type Rect struct{}
func (*Rect) Area() int { return 1 }
func (*Rect) Print() {}
var _ Shape = (*Rect)(nil)

type ValueImpl struct{}
func (ValueImpl) Area() int { return 2 }
var _ Shape = ValueImpl{}

type UniqueWorker struct{}
func (*UniqueWorker) SpecialCustomWork() {}
func (*UniqueWorker) String() string { return "" }
func (*UniqueWorker) SharedOp() {}

type OtherWorker struct{}
func (*OtherWorker) SharedOp() {}

type Factory struct{}
func (Factory) MakeRect() *Rect { return &Rect{} }

type Cfg struct {
	Items []Rect
}

func LinkTarget() {}
"#,
        },
        GoSourceFile {
            path: "use/use.go",
            content: r#"package use

import (
	_ "unsafe"
	"example.com/g3b/iface"
)

//go:linkname localAlias example.com/g3b/iface.LinkTarget
func localAlias()

type Wrapper struct {
	iface.Outer
}

type DupA struct{ iface.Base }
type DupB struct{ iface.Base }
type AmbWrap struct {
	DupA
	DupB
}

type DeepSubIface interface { iface.Shape }

type LocalCfg struct {
	Items []iface.Rect
}

func Callers(o iface.Outer, w Wrapper, aw AmbWrap, s iface.Shape, b iface.Both, sw iface.ShallowWin, cfg LocalCfg, items []*iface.Rect, unk interface{}) (named *iface.UniqueWorker) {
	w.Base.Hello()
	w.MissingPromoted.Hello()
	aw.Base.Hello()
	_ = (((((((((&iface.Rect{}))))))))).Area()
	named.SpecialCustomWork()
	o.Base.Hello()
	s.Area()
	b.Clash()
	sw.DepthWin()

	p := new(iface.Rect)
	p.Print()

	var f iface.Factory
	_ = f.MakeRect().Area()

	for _, item := range cfg.Items {
		item.Area()
	}
	for _, ptr := range items {
		ptr.Print()
	}
	_ = cfg.Items[0].Area()

	if u, ok := unk.(interface{ SpecialCustomWork() }); ok {
		u.SpecialCustomWork()
	}
	if u2, ok := unk.(interface{ String() string; SharedOp() }); ok {
		_ = u2.String()
		u2.SharedOp()
	}
	return named
}
"#,
        },
    ];
    let input = build_input(&files);
    let graph = GoResolver::new().resolve_graph(&input);
    let edges = edge_names(&input, &graph);

    // Tier 4 & field embed
    assert!(edges.contains(&("Callers", "Hello", Reason::TypedMethod)));
    assert!(edges.contains(&("Callers", "DepthWin", Reason::EmbeddedMethod)));
    // Ambiguous same-depth b.Clash() must NOT resolve
    assert!(!edges.iter().any(|(_, dst, _)| *dst == "Clash"));
    // Tier 5 interface call resolves ONLY to Shape.Area (InterfaceMethod), NOT Rect/ValueImpl unless typed concrete call
    let iface_edges: Vec<_> = edges
        .iter()
        .filter(|(_, dst, r)| *dst == "Area" && *r == Reason::InterfaceMethod)
        .collect();
    assert_eq!(iface_edges.len(), 1);
    // Inference: new(Rect), f.MakeRect().Area(), range cfg.Items, cfg.Items[0].Area(), named return
    assert!(edges.contains(&("Callers", "Print", Reason::TypedMethod)));
    assert!(edges.contains(&("Callers", "SpecialCustomWork", Reason::TypedMethod)));
    // Tier 6 UniqueMethod vs stdlib String & ambiguous SharedOp
    assert!(edges.contains(&("Callers", "SpecialCustomWork", Reason::UniqueMethod)));
    assert!(!edges.iter().any(|(_, dst, _)| *dst == "String"));
    assert!(!edges.iter().any(|(_, dst, _)| *dst == "SharedOp"));
    // //go:linkname produces ValueRef with last_segment LinkTarget and NO call edge
    assert!(!edges.iter().any(|(_, dst, _)| *dst == "LinkTarget"));
    assert!(
        input.refs.iter().any(|r| r.kind == RefKind::ValueRef
            && r.text == "go:linkname:example.com/g3b/iface.LinkTarget")
    );

    // Tier 5 links: Rect -> Shape is ImplTrait (1.0), ValueImpl -> Shape is ImplTrait (1.0), Rect -> Printer is StructuralImpl (0.85), no Empty impls
    let shape_id = input.symbols.iter().find(|s| s.name == "Shape").unwrap().id;
    let empty_id = input.symbols.iter().find(|s| s.name == "Empty").unwrap().id;
    let rect_id = input.symbols.iter().find(|s| s.name == "Rect").unwrap().id;
    let printer_id = input
        .symbols
        .iter()
        .find(|s| s.name == "Printer")
        .unwrap()
        .id;
    assert!(graph.links.iter().any(|l| {
        l.kind == phantom_fractal_lang::LinkKind::Impl
            && l.source == phantom_fractal_lang::Node::Symbol(rect_id)
            && l.target == phantom_fractal_lang::Node::Symbol(shape_id)
            && l.reason == phantom_fractal_lang::LinkReason::ImplTrait
            && (l.confidence - 1.0).abs() < 1e-6
    }));
    assert!(!graph.links.iter().any(|l| {
        l.kind == phantom_fractal_lang::LinkKind::Impl
            && l.source == phantom_fractal_lang::Node::Symbol(rect_id)
            && l.target == phantom_fractal_lang::Node::Symbol(shape_id)
            && l.reason == phantom_fractal_lang::LinkReason::StructuralImpl
    }));
    assert!(graph.links.iter().any(|l| {
        l.kind == phantom_fractal_lang::LinkKind::Impl
            && l.source == phantom_fractal_lang::Node::Symbol(rect_id)
            && l.target == phantom_fractal_lang::Node::Symbol(printer_id)
            && l.reason == phantom_fractal_lang::LinkReason::StructuralImpl
            && (l.confidence - 0.85).abs() < 1e-6
    }));
    assert!(
        !graph
            .links
            .iter()
            .any(|l| l.target == phantom_fractal_lang::Node::Symbol(empty_id))
    );
    assert!(
        !graph
            .links
            .iter()
            .any(|l| l.kind == phantom_fractal_lang::LinkKind::Embeds
                && l.source == phantom_fractal_lang::Node::Symbol(rect_id))
    );
}

#[test]
fn bounds_embed_depth_and_handles_one_mb_chained_embed_promptly() {
    // 18-hop embed exceeds MAX_EMBED_DEPTH = 16, while 16-hop succeeds.
    let mut chain = String::from(
        "package chain
type T0 struct{}
func (T0) BaseM() {}
",
    );
    for i in 1..=20 {
        chain.push_str(&format!(
            "type T{i} struct{{ T{} }}
",
            i - 1
        ));
    }
    chain.push_str(
        "func Use(a T16, b T18) { a.BaseM(); b.BaseM() }
",
    );
    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/chain
go 1.26.0
",
        },
        GoSourceFile {
            path: "chain.go",
            content: &chain,
        },
    ];
    let inp = build_input(&files);
    let (edges, _) = GoResolver::new().resolve(&inp);
    assert_eq!(
        edges
            .iter()
            .filter(|e| e.reason == Reason::EmbeddedMethod)
            .count(),
        1
    );

    // >1 MB tree of deep embeds + interfaces resolves quickly (<2s).
    let mut big = String::from(
        "package big
type Root struct{}
func (Root) RMethod() {}
type RIface interface { RMethod() }
",
    );
    for i in 0..800 {
        let prev = if i == 0 {
            "Root".to_owned()
        } else {
            format!("S{}", i - 1)
        };
        big.push_str(&format!(
            "type S{i} struct {{ {prev} }}
func (S{i}) M{i}() {{}}
"
        ));
    }
    big.push_str("// ");
    while big.len() < 1_050_000 {
        big.push_str("padding ");
    }
    big.push('\n');
    let big_files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/big
go 1.26.0
",
        },
        GoSourceFile {
            path: "big.go",
            content: &big,
        },
    ];
    let start = std::time::Instant::now();
    let big_inp = build_input(&big_files);
    let g = GoResolver::new().resolve_graph(&big_inp);
    assert!(start.elapsed().as_secs_f64() < 5.0);
    assert!(!g.links.is_empty());
}

#[test]
fn isolates_inference_and_depth_cap_mutants_with_ambiguous_method_names() {
    let files = [
        GoSourceFile {
            path: "go.mod",
            content: "module example.com/infmut\ngo 1.26.0\n",
        },
        GoSourceFile {
            path: "m/m.go",
            content: r#"package m

type TwoA struct{}
func (*TwoA) Shared() *TwoB { return &TwoB{} }
func (*TwoA) LeafM() {}

type TwoB struct{}
func (*TwoB) Shared() *TwoA { return &TwoA{} }
func (*TwoB) LeafM() {}

type BaseX struct{}
func (*BaseX) Shared() {}
type MidX struct{ BaseX }
type TopX struct{ MidX }

type Chain struct{ F *Chain }
func (*Chain) Self() *Chain { return &Chain{} }
func (*Chain) LeafM() {}

func UseNew() {
	p := new(TwoA)
	p.Shared()
}

func UseShadowedNew() {
	new := func(_ TwoA) TwoB { return TwoB{} }
	p := new(TwoA{})
	p.LeafM()
}

func UseOtherIdentCall() {
	q := notNew(TwoA)
	q.LeafM()
}

func UseIndex(xs []*TwoA) {
	xs[0].Shared()
}

func UseRange(xs []*TwoA) {
	for i, x := range xs {
		_ = i
		x.Shared()
	}
}

func UseRangeIdxOnly(xs []*TwoA) {
	for i := range xs {
		_ = i
	}
}

func UseChain(a *TwoA) {
	a.Shared().Shared().LeafM()
}

func UseDeepParens(a *TwoA, c *Chain, xs []*Chain) {
	_ = (&(&(&(&a)))).LeafM
	((((a)))).Shared()
	// Exceed MAX_HINT_DEPTH = 8 on EVERY compound kind (paren, unary, index, selector, mret):
	((((((((a)))))))).LeafM()
	(&(&(&(&(&(&(&(&a)))))))).LeafM()
	(((((((xs)))))))[0].LeafM()
	c.F.F.F.F.F.F.F.F.LeafM()
	c.Self().Self().Self().Self().Self().Self().Self().Self().LeafM()
}

func UseField(t TopX) {
	t.BaseX.Shared()
}
"#,
        },
    ];
    let input = build_input(&files);
    let graph = GoResolver::new().resolve_graph(&input);
    let edges = edge_names(&input, &graph);

    let two_a_shared = input
        .symbols
        .iter()
        .find(|s| s.owner.as_deref() == Some("TwoA") && s.name == "Shared")
        .unwrap()
        .id;
    let two_a_leaf = input
        .symbols
        .iter()
        .find(|s| s.owner.as_deref() == Some("TwoA") && s.name == "LeafM")
        .unwrap()
        .id;
    let base_shared = input
        .symbols
        .iter()
        .find(|s| s.owner.as_deref() == Some("BaseX") && s.name == "Shared")
        .unwrap()
        .id;

    let caller_hits = |caller: &str| -> Vec<(phantom_fractal_lang::Id, Reason)> {
        let cid = input.symbols.iter().find(|s| s.name == caller).unwrap().id;
        graph
            .calls
            .iter()
            .filter(|e| e.source == cid)
            .map(|e| (e.target, e.reason))
            .collect()
    };

    assert_eq!(
        caller_hits("UseNew"),
        vec![(two_a_shared, Reason::TypedMethod)]
    );
    assert!(caller_hits("UseShadowedNew").is_empty());
    assert!(caller_hits("UseOtherIdentCall").is_empty());
    assert_eq!(
        caller_hits("UseIndex"),
        vec![(two_a_shared, Reason::TypedMethod)]
    );
    assert_eq!(
        caller_hits("UseRange"),
        vec![(two_a_shared, Reason::TypedMethod)]
    );
    assert!(caller_hits("UseRangeIdxOnly").is_empty());
    assert!(edges.contains(&("UseChain", "LeafM", Reason::TypedMethod)));
    assert_eq!(
        caller_hits("UseField"),
        vec![(base_shared, Reason::TypedMethod)]
    );

    let parens_hits = caller_hits("UseDeepParens");
    assert!(parens_hits.contains(&(two_a_shared, Reason::TypedMethod)));
    let chain_leaf = input
        .symbols
        .iter()
        .find(|s| s.owner.as_deref() == Some("Chain") && s.name == "LeafM")
        .unwrap()
        .id;
    assert!(
        !parens_hits
            .iter()
            .any(|(t, _)| *t == two_a_leaf || *t == chain_leaf)
    );

    let lang = GoLanguage::new();
    let mut unary8 = String::from(
        "package d
type C struct{}
func (C) M() {}
func (C) Self() C { return C{} }
func D(a ********C, c C) {
	(********a).M()
	c",
    );
    for _ in 0..8 {
        unary8.push_str(".Self()");
    }
    unary8.push_str(
        ".M()
}
",
    );
    let ext = lang.extract(&unary8).unwrap();
    let m_calls: Vec<_> = ext
        .refs
        .iter()
        .filter(|r| r.kind == RefKind::MethodCall && r.text == "M")
        .collect();
    assert_eq!(m_calls.len(), 2);
    assert!(
        m_calls[0].receiver_type.is_none(),
        "unary8: {:?}",
        m_calls[0]
    );
    assert!(
        m_calls[1].receiver_type.is_none(),
        "mret8: {:?}",
        m_calls[1]
    );
}
