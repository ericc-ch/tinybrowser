# 06: Dom declares Send, not Sync

Type: grilling

Question: Which thread-permission stamps does `Dom` declare?

Answer:

**`Send` only** (auto-derived; no unsafe impls, no locks).

- Legal: building a `Dom` on one worker and handing it to another (future CDP answer path).
- Compiler-forbidden: simultaneous access from two threads.
- Rationale: engine shape is one worker per page (parse → JS → report, sequential); QuickJS runtimes are single-threaded anyway; shared access buys nothing today and would tax every operation forever. Silence on `Sync` is the deliberate, enforced choice.
