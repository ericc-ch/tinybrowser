# 08: Size checkpoint — probe binary + docs/size-budget.md update at milestone close

Type: task

Question: How does dom's milestone get size-checked?

Answer:

When `dom` v1 tests go green: build a throwaway probe binary that genuinely exercises the layer (parse a real-world page, run selector queries against the result), compile with the tuned profile (`opt-level = "z"`, fat LTO, `codegen-units = 1`, stripped), and record the marginal delta vs an empty-`main` build in `docs/size-budget.md`'s measured table. This reproduces the methodology behind every number already in that doc (see removed `sizeprobe`, commit e992a36). Any regression vs the html5ever + selectors estimates (~941 KB + ~115 KB default / tuned equivalents) must justify itself in bytes or shrink.
