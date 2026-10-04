# Decisions

Append-only. Newest last. Each entry: date, context, decision.

## 2026-10-04: WebIDL migration merged

Context: PR #39, 2,887-test slice clean, 4 regressions fixed.
Decision: plain merge; keep Element shim and sync media approximation.
Deferred: generator latent bugs (fix on corpus hit), `scrollIntoView` arg,
reflection shim removal, media stable-state timing.

## 2026-10-04: No hard size cap

Context: 8.7MB shipped; Blitz parity costs +2.9MB.
Decision: minimize stripped x86_64 size, no ceiling. Track in `progress.md`.

## 2026-10-04: Flat docs

Context: `researches/` history went stale; `HANDOFF.md` was a dated log.
Decision: top-level concise docs only; decisions append here.
