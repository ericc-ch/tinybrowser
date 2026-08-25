# 03: JS-facing transport home

Type: grilling

Question: Where do the transport trait and its plain request/response types
live, such that `js` never depends on `net`, and how does the wiring happen?

Answer:

- **Trait + types defined in `js`; implementation in `browser`**
  (option a). `js` owns `trait HttpTransport` plus DOM-flavored plain
  structs (`JsHttpRequest`, `JsHttpResponse`) — mirroring Gecko, whose
  `dom/fetch/InternalRequest`/`InternalResponse` are DOM types, never
  necko types. `browser` implements the trait over `net::Agent` and is the
  only place that sees both worlds (ADR 0001 fan-in rule).
- **Precedent**: the consumer-side-trait inversion is the same pattern as
  html5ever's `TreeSink` implemented by our `Sink` (ADR 0003) — lower
  layer declares the interface, upper layer fills it in.
- **Mechanics**:
  - Trait method is sync/blocking; promise settlement is `js` internals
    after `send()` returns. No async anywhere (whole-stack no-tokio rule).
  - `fetch()` and `XMLHttpRequest` both ride the one trait; WebSocket gets
    its own injection slot when its shape decision lands.
  - Injection once at runtime construction: `js::Runtime::new(transport:
    Box<dyn HttpTransport>)` — js holds an anonymous implementor because
    naming browser's type would be an upward edge.
