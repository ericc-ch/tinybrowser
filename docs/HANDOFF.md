# Handoff (2026-10-13)

Goal: Extract the realm-free DOM layer out of the JS bindings. Move every DOM algorithm into a new `crates/renderer/src/dom/` layer, leave `crates/renderer/src/js/` with the WebIDL surface (the `JsNode` payload, generated contracts, prototypes, and value conversion), and split `js/bindings/node.rs` (12,070 lines) along the way. Land candidate 2 (webidl-bindgen) in a later pass; its findings are recorded below so they do not get lost.

Plan: Layering `documents` + `dom` <- `js` <- `document` <- `engine`. The rule: below `js/` there is no `Ctx`, no `Value`, no `Persistent`, and no `JsNode`. `dom/` takes `&Parsed`/`&mut Parsed`, `DocumentStore`, `World` side data, and `NodeId`s. Tree algorithms return ordered `Effect` values that the binding layer applies in spec order. Seven steps live under Next; each ends with a check. WPT runs for the slices named in the steps are approved (2026-10-13).

State: not started. Nothing in the plan has been written yet. Repo is at `d78b30e` on `main`. No branch created. Baseline checks not yet run for this work.

Done:

- (nothing for this work yet)

Unfinished:

- **Step 1**: move `NodeId` (js/world.rs:271-308), `BlitzId` (js/world.rs:9), `JournalEntry` (js/world.rs:2111), `AttrState`/`AttributeRegistry`/`AttrAttachError` (js/world.rs:2190-2225), and the tree walks `is_connected` (311), `is_iframe_element` (324), `child_ids` (335), `attr` (367), namespaces (403-415), `is_html_tag` (418), `template_contents` (435), `is_html_element` (447), `first_element_with_id` (460), `form_owner_of` (496), `ancestor_distance` (2171), `node_parent` (1827) into `dom/`. Re-export from `js::world` so the 718 `NodeId` references across 24 files keep compiling, then update the five non-`js` importers (`render/mod.rs`, `engine.rs`, `document/mod.rs`, `messaging.rs`, `documents.rs`) to import from `dom`. Check: `tools/check` and `cargo test --workspace`.
- **Step 2**: move `HtmlOutput` (bindings/node.rs:3101-3118), `serializes_as_void` (3119), `push_html_element_name` (3148), `push_html_attribute_name` (3165), `push_escaped_html_text` (3190), `push_escaped_html_attribute` (3202), `serialize_html_element` (3215), `serialize_html_node` (3262), `serialize_html_children` (3351), `serialize_html_outer` (3364), `push_escaped_xml_*` (3375, 3386), `serialize_xml_element` (3405), `serialize_xml_node` (3450), `serialize_xml_children` (3527) to `dom/serialization.rs`. Move `EqualView`/`EqualKind`/`equal_view`/`kinds_equal`/`nodes_equal` (2910-3100) to `dom/equality.rs`. Fix the `parsing.rs:356-357` caller. Check: `tools/check`, `cargo test --workspace`.
- **Step 3**: move the insert/replace/move core (`InsertedNode` 372, `classify_inserted` 390, `ensure_parented` 444, `ensure_no_cycle` 467, `ensure_document_content_model` 507, doctype helpers 581-698, `compare_node_position` 698, `unlink_journaled` 1421, `place_journaled` 1451, `insert_parsed` 1501, `MoveTarget` 1524, `move_before_node` 1539, `ensure_can_move` 1597, `splice_parsed` 1740, `replace_parsed` 1818, `replace_all_with_node` 1917, `replace_all_journaled` 1955, `insert_converted_node` 2005, `insert_tree_node(_inner)` 2013-2110, `insertion_tree_nodes` 229, `insertion_excluding*` 241-253, `siblings_around` 1367) into `dom/insertion.rs`, and `clone.rs` + `adopt_node` (1354)/`adopt_across_documents` into `dom/adoption.rs`. Introduce the ordered `Effect` list for `run_html_insertion_steps` (2111), `clear_nonce_on_connect` (2169), `register_inserted_iframes` (2261), `fixup_option_on_insert` (2421), `fixup_radio_on_insert` (2459), `fixup_focus_after_removal`, `schedule_mutation_delivery`. Check: `tools/check`, `cargo test --workspace`, then the approved WPT slices `dom/nodes` and the collections insertion tests.
- **Step 4**: move `select_options_in` (2530), `option_select_owner` (2558), `option_value_in` (2578), `option_text_in` (2585), `collapse_option_text` (2613), `select_value_in` (2634), `control_value_len` (2646), selection helpers (2674-2738), and the form parts of `bindings/forms.rs` to `dom/forms.rs`. Move `bindings/inner_text.rs` (rendered text collection) and `descendant_text` (bindings/mod.rs:1533) to `dom/text.rs`. Check: `tools/check`, `cargo test --workspace`.
- **Step 5**: split the rest of `bindings/node.rs` into `bindings/node/{mod,core,document,element,html_elements,forms,media}.rs` with the 88 generated trait impls (node.rs:7242-12070) and the 171 method bodies (3626-7224) in their family modules. Shrink the forced `#[rquickjs::methods]` block to the constructor plus `scripts`, `adoptNode`, `innerText`/`outerText` get+set (the only 6 non-`#[qjs(skip)]` members; the macro allows one block per class, `third_party/rquickjs/macro/src/methods.rs:284-330`). Check: `tools/check`, `cargo test --workspace`.
- **Step 6**: migrate those 6 members to the IDL contract path with IDL names (`get_scripts`, `adopt_node`, `get_inner_text`/`set_inner_text`, `get_outer_text`/`set_outer_text`). The shapes are already supported (`[LegacyNullToEmptyString] DOMString` html.idl:131, `readonly attribute HTMLCollection scripts` html.idl:73, `Node adoptNode(Node node)` dom.idl:298). Then delete `brands.js:2` (`native`) and `brands.js:163-255` (`interfaceMembers`), and the empty `block` dispatch in step 5's leftover. Check: `tools/check`, `cargo test --workspace`, then the approved WPT slices for IDL harness and innerText/adoptNode/scripts.
- **Step 7**: cut the `js` <-> `document` module cycle (`js/world.rs:15` imports `document::{Document, FrameRuntime}`; `document/mod.rs:22` imports `js::{...World}`) by moving `FrameRuntime` (document/mod.rs:172-187) and the shared realm state both need to a neutral module. Add the boundary grep to `tools/check` (fail if `dom/` or `documents.rs` mention `Ctx`, `Value`, `Persistent`, `JsNode`). Check: `tools/check`, `cargo test --workspace`.

Next (candidate 2, later pass): webidl-bindgen has one IDL-to-Rust type mapping written three times. `contracts.rs:2366` (`native_type`, plus `argument_parameter:1416`, `attribute_result:1731`, `operation_result:1488`, `union_result:1536`), `emit.rs` per-context conversion (`getter_dispatch:721`, `setter_value_conversion:879`, `reflect_setter:996`, `converted_argument:1205`, `operation_argument_at:1260`; contracts.rs:1413-1415 admits "These mirror the conversions in `emit`"), and `javascript.rs:386-456` for the JS side. `model::Interface` (model.rs:22) has 22 fields and carries pre-rendered tokens (`contract: Option<TokenStream>`, model.rs:23). Module names are computed twice (contracts.rs:333, emit.rs:1790); weedle is parsed in `database.rs`, `model.rs`, and `contracts.rs`. Silent fallbacks: `hook_members` (database.rs:360-371) swallows errors into an empty result, `lower_members` skips unimplemented members (contracts.rs:596-600, 640-648), `elect` returns `Ok(())` when no payload owns the prototype (contracts.rs:164-166), `legacy_codes` always emits nothing (emit.rs:2013-2015), and eight `unreachable!` sites (emit.rs:740, 795-813, 941, 1033, 1051, 1256, 1362, 1680). `validate_sources` (lib.rs:27) has no production caller. Redesign direction: one lowering pass whose conversions are a closed enum, with the trait contract, the dispatch, and the JS-shim check all rendering from it. Checks: `cargo test -p webidl-bindgen` (tests/contracts.rs), `tools/check`, `tools/webidl/import --check`.

Carried over from the 2026-10-10 note (WPT work, still open):

- Explicit manifest step: write `tools/wpt/manifest.py` calling `load_and_update` (third_party/wpt/tools/manifest/manifest.py:365) with the venv python, snapshot `manifest_token` first, stamp only on exit 0.
- `createEvent` init-path proof: read `initEvent`/`initUIEvent` in `js/events.rs` against `__tbEventEntry`, then run the `dom/events` createEvent cases. Needs an approved WPT run.
- Per-move suppression: `js/scripts/web/custom_elements.js:88-106,378-390`; two synchronous `moveBefore()` calls on one node cause spurious disconnect+connect. Gate on the `moveBefore` slice (15 known timing flakes).
- `dom/nodes` fail-list: 59 unexpected + 8 error at 287/354 stable. Re-derive from a fresh `tools/wpt/rerun` before working.

Decisions made:

- Candidate 1 is a layering fix, not a file split. Tree algorithms move below the bindings; the binding layer keeps trait impls, prototypes, and value conversion.
- Below `js/`: no `Ctx`, no `Value`, no `Persistent`, no `JsNode`.
- Effects are an ordered data enum applied by the binding layer, not a callback trait.
- Moved items stay `pub(crate)`. No external API change.
- No new cargo tests for web-platform behavior (WPT owns that). No fork change: the `#[rquickjs::methods]` macro is used as-is.
- `locate_namespace`/`locate_prefix` keep the spec-correct copy. The duplicate in `bindings/attributes.rs:1959-2072` goes away.

Gotchas:

- No host `python3` and no host `pkg-config`/OpenSSL. Run cargo and all runners through `nix develop --command`.
- `tools/wpt/cli.py` slow-path signal semantics are load-bearing. Test runner edits with the stub harness pattern, not live runs.
- `brands.js` fail-fast was rejected once (broke 4 realm tests). Silent skip is correct for script-implemented members.
- `docs/progress.md`: replace binary size, total, and scored groups/slices only.
- The `#[rquickjs::methods]` block emits `use super::JsNode;` in each generated module, so every `include!(concat!(env!("OUT_DIR"), "/<Interface>.rs"))` must sit in a module that has `JsNode` in scope.
- Scratch probes go under `/tmp/opencode/` or `third_party/wpt/`, and WPT scratch files are deleted after use.
