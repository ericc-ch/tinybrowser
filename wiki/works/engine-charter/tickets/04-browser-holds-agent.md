# 04: Browser holds Agent

Type: interview

Question: Is page HTTP a `HttpTransport` trait defined in `js`, or does `browser` hold a `net::Agent`?

Answer:

`browser` holds `net::Agent`. No `HttpTransport` trait.

Fetch, XHR, and WebSocket on the page are host functions that call that agent (`spawn_blocking` around `send` / `upgrade`, per [Event loop](./02-event-loop.md)). Relative URLs and `<base>` resolve in `browser` before `net` sees an absolute URL (already decided on the old net map; still true).

The trait existed to keep `js` from importing `net`. There is no `js` crate yet, and that law is gone. A second implementor does not exist. Tests use a real `Agent` against loopback.

If a fake transport becomes painful later, add a small trait then, at the `browser` boundary, not as a constitutional type in a JS crate.
