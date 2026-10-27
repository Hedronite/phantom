//! `crate-path` edges into workspace libs (SPEC-w4 G2b).

mod support;

use phantom_fractal_lang::{Id, Reason, ResolveCounts, SymbolKind, TargetKind};
use support::{Input, edge, only_edge};

const CORE: &str = "core-lib/src/lib.rs";
const UTIL: &str = "core-lib/src/util.rs";

/// Package `pkg` (lib `src/lib.rs`) depending on `core-lib`, which has
/// `f`, `util::g`, a re-export `again` of `util::g`, and `Thing::new`.
struct Core {
    i: Input,
    caller: Id,
    f: Id,
    g: Id,
    new: Id,
}

fn core() -> Core {
    let mut i = Input::new();
    let root = i.target("core-lib", TargetKind::Lib, CORE);
    i.module(root, "util", UTIL, "");
    let f = i.symbol(SymbolKind::Function, "f", CORE, "");
    let g = i.symbol(SymbolKind::Function, "g", UTIL, "");
    i.symbol(SymbolKind::Struct, "Thing", CORE, "");
    let new = i.method_in(CORE, "Thing", None, "new");
    i.uses(CORE, "", "util::g", Some("again"), false);
    i.dep("pkg", "core-lib");
    let caller = i.function("caller");
    Core {
        i,
        caller,
        f,
        g,
        new,
    }
}

#[test]
fn a_crate_path_reaches_an_item_a_nested_item_a_reexport_and_a_method() {
    let Core {
        mut i,
        caller,
        f,
        g,
        new,
    } = core();
    let direct = i.call(caller, "core_lib::f");
    let nested = i.call(caller, "core_lib::util::g");
    let reexport = i.call(caller, "core_lib::again");
    let method = i.call(caller, "core_lib::Thing::new");
    let turbofish = i.call(caller, "core_lib::Thing::<u8>::new");
    i.call(caller, "core_lib::missing");
    i.call(caller, "core_lib::util::missing");
    i.call(caller, "::core_lib::f");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(direct, caller, f, Reason::CratePath),
            edge(nested, caller, g, Reason::CratePath),
            edge(reexport, caller, g, Reason::CratePath),
            edge(method, caller, new, Reason::CratePath),
            edge(turbofish, caller, new, Reason::CratePath),
        ]
    );
    assert_eq!(
        counts,
        ResolveCounts {
            edges: 5,
            unresolved: 2,
            external: 1,
            ..ResolveCounts::default()
        }
    );
}

#[test]
fn a_crate_path_through_a_use_is_still_crate_path() {
    let Core {
        mut i,
        caller,
        f,
        g,
        new,
    } = core();
    i.uses("src/lib.rs", "", "core_lib::f", None, false);
    i.uses("src/lib.rs", "", "core_lib::util::g", Some("gee"), false);
    i.uses("src/lib.rs", "", "core_lib::util", None, false);
    i.uses("src/lib.rs", "", "core_lib::Thing", None, false);
    let plain = i.call(caller, "f");
    let aliased = i.call(caller, "gee");
    let module = i.call(caller, "util::g");
    let assoc = i.call(caller, "Thing::new");
    let local_path = i.call(caller, "crate::util::g");
    let (edges, _) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(plain, caller, f, Reason::CratePath),
            edge(aliased, caller, g, Reason::CratePath),
            edge(module, caller, g, Reason::CratePath),
            edge(assoc, caller, new, Reason::CratePath),
            edge(local_path, caller, g, Reason::CratePath),
        ]
    );
}

#[test]
fn a_local_name_wins_over_the_crate_name() {
    // A child module named like the crate.
    let Core { mut i, caller, .. } = core();
    i.module(1, "core_lib", "src/core_lib.rs", "");
    let mine = i.symbol(SymbolKind::Function, "f", "src/core_lib.rs", "");
    let call = i.call(caller, "core_lib::f");
    assert_eq!(only_edge(&i), Some(edge(call, caller, mine, Reason::Local)));

    // A `use` binding named like the crate.
    let Core { mut i, caller, .. } = core();
    i.module(1, "other", "src/other.rs", "");
    let mine = i.symbol(SymbolKind::Function, "f", "src/other.rs", "");
    i.uses("src/lib.rs", "", "crate::other", Some("core_lib"), false);
    let call = i.call(caller, "core_lib::f");
    assert_eq!(
        only_edge(&i),
        Some(edge(call, caller, mine, Reason::UseAlias))
    );

    // A local function, a `use` that names nothing, and a glob.
    let setups: [fn(&mut Input); 3] = [
        |i: &mut Input| {
            i.function("core_lib");
        },
        |i: &mut Input| i.uses("src/lib.rs", "", "serde::core_lib", None, false),
        |i: &mut Input| i.uses("src/lib.rs", "", "crate::prelude", None, true),
    ];
    for setup in setups {
        let Core { mut i, caller, .. } = core();
        setup(&mut i);
        i.call(caller, "core_lib::f");
        assert_eq!(only_edge(&i), None);
    }
}

#[test]
fn only_a_dependency_or_a_bin_on_its_own_lib_may_call_a_crate() {
    let Core {
        mut i, caller, f, ..
    } = core();
    let helper = i.function("helper");
    // A lib naming itself.
    i.call(caller, "pkg::helper");
    // The bin of `pkg` may call its own lib and `pkg`'s dependencies.
    i.target("pkg", TargetKind::Bin, "src/main.rs");
    let main = i.symbol(SymbolKind::Function, "main", "src/main.rs", "");
    let own = i.call_from(main, "src/main.rs", "pkg::helper");
    let dep = i.call_from(main, "src/main.rs", "core_lib::f");
    // Another package's bin, with no dependency.
    i.target("tool", TargetKind::Bin, "tool/src/main.rs");
    let tool = i.symbol(SymbolKind::Function, "main", "tool/src/main.rs", "");
    i.call_from(tool, "tool/src/main.rs", "core_lib::f");
    i.call_from(tool, "tool/src/main.rs", "pkg::helper");
    // A dependency on a package with no lib, and two libs with one name.
    i.target("bin-only", TargetKind::Bin, "bin-only/src/main.rs");
    i.dep("pkg", "bin-only");
    i.call(caller, "bin_only::run");
    i.target("a-b", TargetKind::Lib, "a-b/src/lib.rs");
    i.target("a_b", TargetKind::Lib, "a_b/src/lib.rs");
    i.symbol(SymbolKind::Function, "x", "a-b/src/lib.rs", "");
    i.symbol(SymbolKind::Function, "x", "a_b/src/lib.rs", "");
    i.dep("pkg", "a-b");
    i.dep("pkg", "a_b");
    i.call(caller, "a_b::x");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(own, main, helper, Reason::CratePath),
            edge(dep, main, f, Reason::CratePath),
        ]
    );
    assert_eq!(counts.external, 5);
}

#[test]
fn self_calls_on_another_crates_type_stay_in_the_callers_target() {
    let Core { mut i, .. } = core();
    i.method_in(CORE, "Thing", None, "name");
    i.method_in(CORE, "Thing", None, "label");
    let show = i.method_in("src/lib.rs", "core_lib::Thing", Some("Show"), "show");
    i.method_in("src/lib.rs", "core_lib::Thing", Some("Label"), "label");
    let to_own = i.method_call(Some(show), "self", "show");
    // Only `core-lib` has `name`; both crates have a `label`.
    i.method_call(Some(show), "self", "name");
    i.method_call(Some(show), "self", "label");
    let (edges, counts) = i.resolve();
    assert_eq!(edges, [edge(to_own, show, show, Reason::SelfMethod)]);
    assert_eq!(counts.unresolved, 2);
}
