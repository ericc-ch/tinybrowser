# Handoff (2026-09-20)

State: the MDN form dogfood slice is verified on this working tree and ready to land.
The exact MDN page fills, and `/tmp/tinybrowser-mdn-filled.png` shows `Tiny Browser` in the name input.
Repo-local checks are green: `tools/check` and `cargo test --workspace` passed.

Done:

- Focused WPT 6/6 via `tools/wpt/run --exclude=worker` on `module-vs-script-1.html`, `imports.html`, `currentScript-null.html`, `Element-interface-attachShadow.html`, `WebCryptoAPI/getRandomValues.any.js`, and `randomUUID.https.any.js`.
- Playwright gate: 9 passed, 1 skipped textarea `fixme` (`tools/playwright/run`).
- MDN dogfood: `TINYBROWSER_BIN=/mnt/ssd/rust-targets/shared/debug/tinybrowser node tools/playwright/mdn-form.mjs` wrote `/tmp/tinybrowser-mdn-filled.png`.
- `crypto.getRandomValues` over the 65536-byte quota now throws `QuotaExceededError` (null `quota`/`requested`), not a plain `DOMException`.

In flight:

- This slice is still a local working tree. Open the PR, then resume the broader WPT chase only after it lands.

Next:

1. Land the MDN dogfood PR.
2. Resume the broader WPT chase.

Decisions made:

- Binary-size work remains deferred by explicit user request.
- When `mdnplay.dev` is unavailable, use the exact source read from MDN's live runner component inside the real child frame; never substitute a local fixture.
- Child frame IDs use the explicit nonstandard `<tab-id>.<renderer-frame-id>` extension; omitted `frameId` remains standard main-frame behavior.
- Partial APIs must not fake security or transformation semantics; unsupported crypto algorithms and compression streams stay absent.

Gotchas:

- `mdnplay.dev` currently returns 503, so the dogfood script normally takes its MDN-source fallback.
- Nix sets Cargo's target directory to `/mnt/ssd/rust-targets/shared`, not repository-local `target/`.
- `cargo fmt --all` rewrites unrelated merged files under the current formatter; format only touched files and inspect status.
- Shadow rendering is intentionally partial: tree traversal works, but Stylo still flattens shadow scope and does not collect shadow-local stylesheets.
