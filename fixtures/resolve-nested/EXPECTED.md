# Expected edges — `fixtures/resolve-nested`

Ground truth for nested inner `fn` extraction and resolution (SPEC-w16 G2):
- V08 shape (`is_root` nested inside `nearby_entries` in `src/app.rs`);
- nested `fn step` inside an `impl App` method `App::run`;
- calls from enclosing function/method to nested `fn` and from nested `fn` to module/crate helpers;
- two outer functions (`outer_alpha` and `outer_beta`) defining same-named `fn shared_helper`;
- depth-2 nested-in-nested `fn` (`layered` -> `mid` -> `leaf`).

## Edges

- `lib:crate::app::App::run -> lib:crate::app::App::run::step local`
- `lib:crate::app::App::run::step -> lib:crate::app::aux local`
- `lib:crate::app::nearby_entries -> lib:crate::app::nearby_entries::is_root local`
- `lib:crate::app::nearby_entries::is_root -> lib:crate::app::aux local`
- `lib:crate::layered -> lib:crate::layered::mid local`
- `lib:crate::layered::mid -> lib:crate::layered::mid::leaf local`
- `lib:crate::layered::mid::leaf -> lib:crate::top_helper local`
- `lib:crate::outer_alpha -> lib:crate::outer_alpha::shared_helper local`
- `lib:crate::outer_alpha::shared_helper -> lib:crate::top_helper local`
- `lib:crate::outer_beta -> lib:crate::outer_beta::shared_helper local`
- `lib:crate::outer_beta::shared_helper -> lib:crate::app::aux path-exact`

## Counts

- edges: 11
- unresolved: 0
- external: 0
- no_source: 0
- method_calls: 0
- macros: 0
- modules: 2
- orphans: 0
