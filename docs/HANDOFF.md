# Handoff (2026-09-27)

Goal: land the `<select>` option collection + Window element named access work
(open as PR #35), then fork rquickjs to fix the exotic-class bug that blocks a
native collection implementation, and finally deal with `arena.rs` bloat.

Plan:

1. Get PR #35 reviewed and merged (branch `feat/select-window-named-access`).
2. Fork rquickjs and fix the two exotic-class defects below, then rewrite
   `JsCollection` as an exotic class and delete the JS Proxy.
3. Refactor `crates/dom/src/arena.rs` (move form/named tenants out; consider a
   module split).

State: `tools/check` green; `cargo test --workspace` 33 suites / 0 failures;
Playwright `forms.spec.ts` 3/3. Branch pushed, tree clean. PR #35 is open and
shows `CHANGES_REQUESTED` from CodeRabbit; its one finding is fixed in `11fdadb`
and the thread is answered, but CodeRabbit used up its included review and did
not re-review. No repo-local `doctor` target exists; `tools/check` is the gate.

Done:

- `fe331df` html: live options and selectedOptions collections. Verified: WPT
  `the-select-element` 24/131 -> 32/131 (at that commit); `the-option-element`
  11/11; `tools/check` + `cargo test --workspace` + Playwright green.
- `4852e64` html: option insertion selectedness and select namedItem. Verified:
  fixes `inserted-or-removed.html` and `select-named-getter.html`; `the-select-element`
  37/131 combined at head.
- `d1f71c0` html: element named access on the Window object. Verified:
  `named-access-on-the-window-object` 4/17 -> 10/17.
- `c33a024` html: apply code review to option collections and window named
  access (two subagent reviews). Must-fixes: `ownKeys` Proxy-invariant break
  removed; quadratic name-set rebuild replaced by the `dom/src/named.rs` index;
  insertion/removal routed through `inserted_list_owner`; `HTMLOptionsCollection`
  named properties enumerable; `select.size` reflection range; `select.item`
  Uint32 coercion. Verified: `tools/check`; `cargo test --workspace` (33 ok);
  Playwright 3/3; a webdriver probe confirmed each fix; interleaved appends +
  global misses scale 2k/4k/8k -> 186/366/737 ms (linear).
- `11fdadb` html: bound the Window named-name index (CodeRabbit finding). Verified:
  20,000-distinct-id churn on one element in 200 ms; stale ids resolve to
  `undefined`; `named-access` 10/17 and `the-select-element` 37/131 unchanged.
- Upstream finding (no repo change): rquickjs exotic classes cannot inherit their
  prototype chain. Repro in `/tmp/opencode/exotic-spike` (ephemeral; see Gotchas).

Unfinished:

- PR #35 not merged and the stale `CHANGES_REQUESTED` not dismissed. Resume with
  `gh pr view 35 --comments`; `gh api repos/ericc-ch/tinybrowser/pulls/35/reviews`.
- rquickjs fork not started. Resume point: implement the two fixes below in a
  fork, bump `crates/renderer/Cargo.toml` (`rquickjs = "=0.14.0"` today), then
  rewrite `JsCollection` (`crates/renderer/src/js/bindings/collections.rs:30`)
  as `#[rquickjs::class(exotic)]` + an exotic impl, and delete
  `__tb_liveCollection` / `isPlatformMethod` in
  `crates/renderer/src/js/scripts/collections.js:166-261`. Re-run WPT
  `named-access-on-the-window-object` and `the-select-element`, plus Playwright.
- `arena.rs` refactor not started (see Next 3).

Next:

1. Fork rquickjs; fix `VTable::get` (`rquickjs-core/src/class/ffi.rs:485`) so
   `get_property`/`has_property` are registered only when the class overrides
   them (the macro knows: emit a `const HAS_GET_PROPERTY`/`HAS_HAS_PROPERTY`
   marker). Also stop discarding the object in the `has_property`/
   `delete_property` FFI trampolines (`ffi.rs:390`) and pass it to the trait.
2. Rewrite `JsCollection` exotic, delete the Proxy, re-verify, then move
   `select[index]` and the `Window` prototype chain onto the same footing if the
   identity problem is gone.
3. Refactor `arena.rs`: move `append_blank_options` (`arena.rs:2377`) into
   `form/select.rs` behind one `pub(crate)` arena primitive; move the Window
   named-name index out of `record` into a mutation-hook seam; then split the
   remaining tree/arena code if it still reads as several jobs.

Decisions made:

- Keep the JS Proxy for collections for now. Exotic cannot inherit a prototype
  chain in this rquickjs (evidence below), so it is not a drop-in replacement.
- One PR, not two stacked PRs: the review fixes span the shared collection code,
  so the three commits were combined on `feat/select-window-named-access`.
- `select[index]` (options mirrored on the element) stays out of reach until
  rquickjs gives class instances a non-Proxy identity for `this`.
- Window named access covers elements only (id on any element; `name` on
  `embed`/`form`/`img`/`object`). Navigable names and the `Window.prototype`
  chain need browsing contexts and a `Window` interface.
- The named-name index grows only within a rebuild and is rebuilt from the
  document when churn exceeds twice the live count plus a slack of 1024
  (`crates/dom/src/named.rs`).

Gotchas:

- The rquickjs exotic bug, in full. QuickJS's lookup (`quickjs-sys-0.14.0/quickjs/quickjs.c:9189-9249`)
  calls `em->get_property` and returns immediately if it exists; it only falls
  through to the prototype when using `get_own_property` and that returns 0.
  rquickjs always installs `get_property`/`has_property`
  (`rquickjs-core-0.14.0/src/runtime/exotic.rs:10-18`,
  `src/class/ffi.rs:485`), so any exotic class shadows its whole prototype
  chain. `exotic_has_property`/`exotic_delete_property` also get no object
  (`ffi.rs:390` marks it `_obj`), and `JsCell` exposes only `borrow`/
  `borrow_mut` (`src/class/cell.rs:210-255`), so delegation is impossible from
  the trait. Spike proof: with the macro, `coll.item` is "not a function",
  `'item' in coll` is false, while a non-exotic control class with the same
  methods works and `Object.getOwnPropertyDescriptor(Object.getPrototypeOf(coll), 'item')`
  is defined.
- `/tmp/opencode/exotic-spike` is the only copy of the repro and `/tmp` is
  wiped; recreate from the Gotchas description if needed.
- Build with the shared target dir to reuse the QuickJS C build:
  `CARGO_TARGET_DIR=/mnt/ssd/rust-targets/shared`. Direct cargo/runners must run
  through `nix develop --command`.
- WPT reruns used here:
  `nix develop --command ./tools/wpt/run --score <paths> --save-report <json> -- --exclude=worker --processes 8 --fully-parallel`.
