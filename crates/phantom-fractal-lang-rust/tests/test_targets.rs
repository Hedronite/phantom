//! What a `test` target may call by crate name (SPEC-w11 G1b).

mod support;

use phantom_fractal_lang::{Reason, SymbolKind, TargetKind};
use support::{Input, edge};

const CORE: &str = "core-lib/src/lib.rs";
const DEV: &str = "dev-lib/src/lib.rs";
const IT: &str = "tests/it.rs";

#[test]
fn a_test_calls_its_own_lib_its_dependencies_and_its_dev_dependencies() {
    let mut i = Input::new();
    let helper = i.function("helper");
    i.target("core-lib", TargetKind::Lib, CORE);
    let f = i.symbol(SymbolKind::Function, "f", CORE, "");
    i.dep("pkg", "core-lib");
    i.target("dev-lib", TargetKind::Lib, DEV);
    let probe = i.symbol(SymbolKind::Function, "probe", DEV, "");
    i.dev_dep("pkg", "dev-lib");
    i.target("pkg", TargetKind::Test, IT);
    let t = i.symbol(SymbolKind::Function, "t", IT, "");
    let own = i.call_from(t, IT, "pkg::helper");
    let dep = i.call_from(t, IT, "core_lib::f");
    let dev = i.call_from(t, IT, "dev_lib::probe");
    let local = i.call_from(t, IT, "t");
    let (edges, counts) = i.resolve();
    assert_eq!(
        edges,
        [
            edge(own, t, helper, Reason::CratePath),
            edge(dep, t, f, Reason::CratePath),
            edge(dev, t, probe, Reason::CratePath),
            edge(local, t, t, Reason::Local),
        ]
    );
    assert_eq!(counts.external, 0);
}

#[test]
fn only_a_test_of_the_package_takes_a_dev_dependency() {
    let mut i = Input::new();
    let caller = i.function("caller");
    i.target("dev-lib", TargetKind::Lib, DEV);
    i.symbol(SymbolKind::Function, "probe", DEV, "");
    i.dev_dep("pkg", "dev-lib");
    // The lib and the bin of `pkg` ignore the dev entry.
    i.call(caller, "dev_lib::probe");
    i.target("pkg", TargetKind::Bin, "src/main.rs");
    let main = i.symbol(SymbolKind::Function, "main", "src/main.rs", "");
    i.call_from(main, "src/main.rs", "dev_lib::probe");
    // Another package's test has neither `pkg`'s dev entry nor its lib.
    let other = "other/tests/t.rs";
    i.target("other", TargetKind::Test, other);
    let t = i.symbol(SymbolKind::Function, "t", other, "");
    i.call_from(t, other, "dev_lib::probe");
    i.call_from(t, other, "pkg::caller");
    let (edges, counts) = i.resolve();
    assert!(edges.is_empty(), "{edges:?}");
    assert_eq!(counts.external, 4);
}
