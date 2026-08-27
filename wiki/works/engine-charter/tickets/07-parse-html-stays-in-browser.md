# 07: parse_html stays in browser

Type: interview

Question: Does `parse_html` stay in `browser` once that crate is also the page actor?

Answer:

Yes. `browser` is the engine crate: TreeSink today, page lifecycle later. No fifth crate. Template contents move onto `Dom` ([Template contents on Dom](./03-template-contents-on-dom.md)); the sink stays in `browser`. Root `tinybrowser` depends on `browser` only ([Crate graph](./01-crate-graph.md)).
