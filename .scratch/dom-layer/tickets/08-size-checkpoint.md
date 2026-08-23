# 08: Size checkpoint — probe binary + docs/size-budget.md update at milestone close

Type: task

Question: How does dom's milestone get size-checked?

Answer:

When `dom` v1 tests go green: build a throwaway probe binary that genuinely exercises the layer (parse a real-world page, run selector queries against the result), compile with the tuned profile (`opt-level = "z"`, fat LTO, `codegen-units = 1`, stripped), and record the marginal delta vs an empty-`main` build in `docs/size-budget.md`'s measured table. This reproduces the methodology behind every number already in that doc (see removed `sizeprobe`, commit e992a36). Any regression vs the html5ever + selectors estimates (~941 KB + ~115 KB default / tuned equivalents) must justify itself in bytes or shrink.

---

*Answered 2026-08-23, at dom v1 close.* Done as written: `.scratch/dom-layer/sizeprobe/` parses a real Wikipedia page (405 KB → 4,051 elements) through a throwaway TreeSink over our arena and runs queries of every common shape; baseline lives beside it. **Marginal: +932 KB tuned / +1272 KB default release** vs +915/+1056 KB estimated — within ~2% tuned; release overhead is dom's own code plus query execution the estimate never carried. Recorded in the size-budget doc's milestone section; chosen-stack headroom now ~2.58 MB of 5 MB. The probe's TreeSink is scaffolding, not the real adapter — parse-correctness testing (ticket 07's html5lib suite) stays with the adapter milestone, exactly as ADR 0003 places parsing above dom.
