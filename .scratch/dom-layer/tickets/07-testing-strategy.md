# 07: Testing — html5lib tree-construction suite plus unit tests

Type: grilling

Question: What is `dom`'s testing strategy?

Answer:

**Parse correctness:** vendor the html5lib tree-construction suite (~5k cases shipped as data files in html5ever's repo) and assert our `Dom` reproduces the spec-mandated tree for every case. This is the same bar production engines use and covers exactly the misnesting/table/unclosed-tag traps hand-written examples miss.

**Unit tests** for everything the suite doesn't see: arena mechanics (stale `NodeId` → clean miss, generation ticks on slot reuse), inline→heap children spill behavior and never-spill-back, bulk moves (`reparent_children`), selector matching against known trees.

Not adopted now: full web-platform-tests corpus (needs harness machinery that belongs to later layers).
