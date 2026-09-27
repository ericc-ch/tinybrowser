# Handoff (2026-09-27)

Goal: three workstreams, in order.

1. Land the `<select>` option collections + Window element named access work
   (open as PR #35).
2. Fork rquickjs to fix the exotic-class defect that makes native collections
   impossible, then rewrite the collection wrappers to use it and drop the JS
   Proxy.
3. Refactor `crates/dom/src/arena.rs`, which has become the god object plus the
   god function again.

Why: (1) is working conformance gains that should not rot on a branch. (2) removes
the Proxy that forces `select[index]` to stay unimplemented and hides the real
cost (every `item`/`length` recomputes the whole collection). (3) is the reason
every feature collides in one file; the last split (`57fd43c`) did not finish the
job.

Plan: land PR #35 first (it is green). Then the rquickjs fork as its own branch,
because it changes a dependency and needs the collection rewrite plus a WPT rerun.
Then the arena refactor as a third branch, because it is mechanical but touches
many call sites.

State: `tools/check` green; `cargo test --workspace` 33 suites / 0 failures;
Playwright `forms.spec.ts` 3/3. Working tree clean. Branch
`feat/select-window-named-access` is pushed; head `a6192a8` (this note) on top of
`11fdadb` (last code commit). PR #35 is open. CodeRabbit left
`CHANGES_REQUESTED` at 2026-09-27T09:05:15Z with one finding, which is fixed in
`11fdadb` and answered on the thread, but CodeRabbit used its included review and
never re-reviewed. There is no `doctor` target; `tools/check` is the gate
(`tools/` holds `check`, `wpt`, `playwright`, `cdp-tests`, `intl`, `release`,
`binary-path`, `coderabbit-wait`).

## Task list

- [ ] Merge PR #35 (or get the stale review dismissed).
- [ ] Fork rquickjs; fix exotic hook registration and the dropped object args.
- [ ] Rewrite `JsCollection` as an exotic class; delete the Proxy; re-run WPT.
- [ ] Move `append_blank_options` out of `arena.rs` behind a tree primitive.
- [ ] Move the Window named-name index behind a mutation-hook seam.
- [ ] Split the remaining `arena.rs` concerns (shadow/lifecycle/attr/text/meta).

Done:

- `fe331df` html: live options and selectedOptions collections. Evidence:
  `tools/check`, `cargo test --workspace`, Playwright green; WPT
  `the-select-element` 24/131 -> 32/131 at that commit; `the-option-element` 11/11.
- `4852e64` html: option insertion selectedness and select namedItem. Evidence:
  fixes WPT `inserted-or-removed.html` and `select-named-getter.html`;
  `the-select-element` 37/131 at head.
- `d1f71c0` html: element named access on the Window object. Evidence: WPT
  `named-access-on-the-window-object` 4/17 -> 10/17.
- `c33a024` html: apply code review to option collections and window named access.
  Fixes: removed the `ownKeys` Proxy-invariant break; replaced the per-mutation
  name-set rebuild with the growth-only index in `dom/src/named.rs`; routed
  insertion/removal through `inserted_list_owner`; made `HTMLOptionsCollection`
  named properties enumerable while `HTMLCollection` stays unenumerable; fixed
  `select.size` reflection range; made `select.item` coerce as `unsigned long`.
  Evidence: `tools/check`; `cargo test --workspace` (33 ok); Playwright 3/3; a
  webdriver probe confirmed each fix; interleaved appends + global misses scale
  2k/4k/8k -> 186/366/737 ms (linear).
- `11fdadb` html: bound the Window named-name index (CodeRabbit finding).
  Evidence: a 20,000-distinct-id churn on one element runs in 200 ms, the current
  id resolves, stale ids resolve to `undefined`, removed elements stop resolving;
  `named-access` 10/17 and `the-select-element` 37/131 unchanged.
- `a6192a8` docs: this handoff.
- Upstream finding, no repo change: rquickjs exotic classes cannot inherit a
  prototype chain. Repro spike was at `/tmp/opencode/exotic-spike` (ephemeral;
  Reference A has the details needed to recreate it).

Unfinished:

- PR #35 not merged; stale `CHANGES_REQUESTED` not dismissed. Resume:
  `gh pr view 35 --comments`; `gh api repos/ericc-ch/tinybrowser/pulls/35/reviews`.
- rquickjs fork not started. No branch yet. Resume at `Next` step 1.
- `arena.rs` refactor not started. No branch yet. Resume at `Next` step 4.
- Nothing is broken or partway in code; both unfinished items are greenfield.

Next:

1. Fork rquickjs. Two code fixes, both in `rquickjs-core`:
   - Make exotic hook registration conditional. The macro knows which hooks were
     written (`#[qjs(get)]`, `#[qjs(has)]`, ... in the `#[rquickjs::exotic]`
     impl); it should emit markers (e.g. `const HAS_GET_PROPERTY: bool`) and
     `VTable::get` should leave the pointer `None` when the hook is absent.
   - Stop discarding the object in the `has_property`/`delete_property`
     trampolines and pass it to the trait, so a class can delegate to its
     prototype for `in` and `delete`.
2. Bump `crates/renderer/Cargo.toml` (`rquickjs = "=0.14.0"` today) to the fork,
   build through `nix develop --command`, and rerun `tools/check`.
3. Rewrite `JsCollection` (`crates/renderer/src/js/bindings/collections.rs:30`)
   as `#[rquickjs::class(exotic)]` plus an exotic impl; delete
   `__tb_liveCollection`/`isPlatformMethod` and the Window named Proxy in
   `crates/renderer/src/js/scripts/collections.js` (`:166-261`, `:296-343`) once
   the identity problem is gone. Then attempt `select[index]`. Rerun WPT
   `named-access-on-the-window-object`, `the-select-element`, and the forms dirs,
   plus Playwright.
4. `arena.rs` step 1 (cheap): move `Dom::append_blank_options` (`arena.rs:2377`)
   into `form/select.rs` behind one `pub(crate)` tree primitive (the method needs
   `insert_linked`, `record`, `require_live`, all private today). Move the Window
   named index off `record` onto a small `hooks.rs` seam.
5. `arena.rs` step 2 (mechanical): lift the independent concerns into modules —
   shadow/template/slots (~14 methods, 3 fields), frame connection/lifecycle
   (~9 methods, 3 fields), attributes (~16), text (~8), document metadata (~12).
   That leaves slots + tree accessors + mutation primitives in `arena.rs`.
6. `arena.rs` step 3 (only if wanted): split `Dom` into sub-structs (`Arena`,
   `Tree`, `Shadow`, `Frames`, `FormState`, `Meta`, `Named`) so the
   `pub(crate)` convention becomes a compiler-enforced boundary.

Decisions made:

- Keep the JS Proxy for collections for now. Exotic cannot inherit a prototype
  chain in rquickjs 0.14 (Reference A), so it is not a drop-in replacement; the
  fork must come first.
- One PR, not two stacked PRs: the review fixes span the shared collection code,
  so all three feature commits live on `feat/select-window-named-access`.
- `select[index]` (options mirrored on the element) stays unimplemented until
  class instances have a non-Proxy identity for `this`.
- Window named access covers elements only: id on any element; `name` on
  `embed`/`form`/`img`/`object`. Navigable names and the `Window.prototype` chain
  need browsing contexts and a `Window` interface.
- The named-name index grows only within a rebuild and is rebuilt from the
  document when churn exceeds twice the live count plus a slack of 1024
  (`crates/dom/src/named.rs`).
- For the arena refactor, prefer exposing one narrow tree primitive over making
  `insert_linked`/`record` `pub(crate)` wholesale; and prefer a small `hooks.rs`
  over a boxed-closure registry (borrow rules fight the latter).

Gotchas:

- All direct cargo/runners go through `nix develop --command`. Reuse the QuickJS C
  build with `CARGO_TARGET_DIR=/mnt/ssd/rust-targets/shared`.
- WPT pattern used here:
  `nix develop --command ./tools/wpt/run --score <paths> --save-report <json> -- --exclude=worker --processes 8 --fully-parallel`.
- The reference sections below are findings only; no code change exists yet.
- `/tmp` is wiped, so the spike and all probe logs are gone. Recreate from the
  references if needed.

## Reference A — the rquickjs exotic-class defect

Symptom: an rquickjs exotic class cannot inherit from its prototype chain. A
method on the prototype is unreachable through the object, and `in` is false for
it.

Evidence from the spike: a fake collection with `length`/`item` on its
(`#[rquickjs::methods]`) prototype, plus exotic hooks for indexed/named
properties, produced `coll[0] === "item0"`, `coll.named === "named-value"`,
`Object.keys(coll) === ["0","1","2","named"]`, but `coll.item` was "not a
function", `coll.length` was `undefined`, and `'item' in coll` was false. A
non-exotic control class with the same methods worked (`plain.item(0)`,
`plain.length`, `'item' in plain`), and
`Object.getOwnPropertyDescriptor(Object.getPrototypeOf(coll), 'item')` was
defined — so the property was present on the prototype and the lookup never
reached it.

Root cause, in three layers:

- QuickJS: `JS_GetPropertyInternal` (`rquickjs-sys-0.14.0/quickjs/quickjs.c:9189-9249`)
  consults `em->get_property` and returns immediately when it exists; only
  `em->get_own_property` can "decline" (return 0) and let the loop continue to
  `p->shape->proto`. `JS_HasProperty` (quickjs.c:9888-9897) and the set path
  (quickjs.c:10654-10662) short-circuit the same way.
- rquickjs always installs those hooks: `VTable::get` sets `get_property`,
  `has_property`, `set_property` unconditionally
  (`rquickjs-core-0.14.0/src/class/ffi.rs:485-490`), and the single
  `JSClassExoticMethods` is built with all of them in
  `src/runtime/exotic.rs:10-18` (`define_own_property: None, // TODO`).
- rquickjs hides the object from the hooks that could delegate:
  `has_property`/`delete_property` discard it (`_obj`, `ffi.rs:390`), the
  `exotic_has_property`/`exotic_delete_property` trait methods take no receiver,
  and `JsCell` exposes only `borrow`/`borrow_mut`
  (`src/class/cell.rs:210-255`). The macro's `#[qjs(get)]` wrapper also drops the
  receiver (`rquickjs-macro-0.14.0/src/exotic.rs`, `expand_wrapper` args are
  `ctx, atom`).

Signatures the macro accepts (verified by compiling the spike): `#[qjs(get)] fn
(&self, [&Ctx,] Atom) -> `{[Result]} IntoJs`; `#[qjs(set)] fn (&mut self, [&Ctx,]
Atom, Value) -> `{[Result]} bool`; `#[qjs(has)]`/`#[qjs(delete)]` like set minus
Value; `#[qjs(get_own_property)] -> `{[Result]} Option<PropertyDescriptor<'js>>`;
`#[qjs(get_own_property_names)] -> `{[Result]} Vec<PropertyName<'js>>`. The exotic
impl block is `#[rquickjs::exotic]`: the crate's own example is
`rquickjs-0.14.0/tests/macros/pass_exotic.rs`, and the trait-level version is
`rquickjs-core-0.14.0/src/class.rs:98-178`. `#[rquickjs::methods]` and
`#[rquickjs::exotic]` compile together on one type; the breakage is runtime, not
syntactic.

The fix (upstream): don't register `get_property`/`has_property` unless the class
overrides them; pass the object to `has`/`delete`. Then WebIDL's
`[[GetOwnProperty]]`-then-prototype path works, and `JsCollection` can be native.

## Reference B — arena.rs inventory and the privacy trap

`arena.rs` is 2,610 lines with **120 `impl Dom` methods** plus the `Dom` struct
(**29 fields**) and the enums `Mutation`, `Lifecycle`, `DomError`, `QuirksMode`,
`Slot`, `Children`. The `impl Dom` blocks are scattered across the crate:
`arena.rs:287`, `selector.rs:967`, `named.rs:21`, `form/input.rs:36`,
`form/mod.rs:14`, `form/select.rs:7` — all one type.

Contents of `arena.rs`, by concern (method counts approximate):

| Concern | ~Methods | Examples |
|---|---|---|
| Slot arena + handles | 9 | `alloc`, `live_slot`, `node_mut`, `destroy`, `would_cycle` |
| Tree accessors | 9 | `kind`, `parent`, `children`, `first_child`, `previous_sibling` |
| Rendered-tree helpers | 4 | `rendered_children`, `rendered_descendants` |
| Node construction + clone | 8 | `create_element`, `clone_node` |
| Shadow DOM / template / slots | 14 | `attach_shadow`, `assigned_nodes`, `template_contents`, `first_slot` |
| Frame connection / lifecycle | 9 | `connection_snapshot`, `is_connected`, `connected_iframe_count` |
| Tree mutation primitives | 15 | `pre_insert`, `place_node`, `splice_fragment`, `unlink`, `replace_all` |
| Attributes | 16 | `set_attribute`, `remove_attribute`, `attribute_names`, `element_mut` |
| Text / character data | 8 | `set_text`, `append_text`, `set_comment`, `set_data` |
| Document metadata | 12 | `quirks_mode`, `document_language`, `scroll_offset`, `active_element` |
| Mutation recorder | 2 | `record`, `mutation_serial` |
| Kind predicates | 5 | `can_contain_children`, `is_insertable`, `is_fragment` |
| Misc | 4 | `incoming_nodes`, `merge_attrs` |
| Form tenant | 1 + call sites | `append_blank_options`; back-calls in `place_node`/`splice_fragment`/`unlink`/`set_attribute` |
| Window named tenant | 0 + a `record` hook | index driven from `record` into `named.rs` |

`Dom`'s 29 fields mix the same concerns: arena (`slots`, `free`, `document`);
metadata (`quirks_mode`, `document_language`, `script_lines`, `active_element`,
`scroll_offsets`); shadow/template (`template_contents`, `shadow_roots`,
`shadow_hosts`); form (`input_values`, `input_types`, `checkedness`,
`indeterminate`, `option_selectedness`, `option_dirty_selected`,
`input_selectable`, `selections`); observer/frames (`mutations`,
`record_mutations`, `recording_suppressed`, `lifecycle`, `connected_iframes`,
`mutation_serial`); Window named (`named_names`, `named_names_seeded`,
`named_names_watermark`).

Why it keeps growing:

- The struct is defined here, so every domain's state is declared here even when
  the algorithms moved to `form/`. `57fd43c` ("dom: split the form model out of
  the arena") explicitly kept the state tables on `Dom` as `pub(crate)`, and left
  the mutation primitives calling back into the form model at the spec's
  phenomena points. That is the documented boundary, and it means new form work
  edits `arena.rs`.
- The mutation primitives are the universal chokepoint (spec insertion steps,
  form-owner changes, option selectedness). Every feature adds calls there.
- Rust inherent impls may live in any module of the type's crate, but a method
  can only use items visible to its module. `append_blank_options` needs
  `insert_linked` (`arena.rs:2254`), `record` (`:523`), and `require_live`
  (`:2412`), all **private** to `arena.rs` (module-based privacy; a file is one
  module here). So it cannot be written in `form/select.rs` today even though
  that file already has `impl Dom`. `create_element` (`:738`) and `last_child`
  (`:626`) are `pub`, `<Dom>` fields such as `option_selectedness` are
  `pub(crate)`, and `apply_default_selectedness`/`inserted_list_owner`/
  `option_added_to_select` are `pub(crate)` — but the three writers above are the
  blockers.

Fix shape for step 4: expose one primitive, e.g.
`pub(crate) fn insert_new_children(&mut self, parent: NodeId, children: &mut Vec<NodeId>, record: bool)`
(or similar), then write `append_blank_options` in `form/select.rs`. The method
placement is cosmetic; the primitive boundary is the real change.

For the Window named index, the seam is a small `hooks.rs` with explicit
`node_inserted`/`node_removed`/`attribute_changed` entry points the primitives
call, rather than a `record` hook that knows about names. A boxed-closure
registry is possible but fights `&mut Dom` borrowing during dispatch.
