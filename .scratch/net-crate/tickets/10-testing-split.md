# 10: Testing split

Type: grilling

Question: How is net tested — at which boundaries, with which tools, and
what deliberately does NOT get tested?

Answer:

Priority order follows code-conventions (public interfaces first):

1. **Loopback end-to-end**: `Agent` against a real `std::net` TCP server
   serving canned HTTP — full request→send→Response through the public
   API only.
2. **Integration seams**: cookie harvest/replay across two sends;
   redirect chains; chunked bodies; drop-cancels contract observed by the
   recording server.
3. **Property tests** (`proptest`, dev-dependency only — zero binary
   cost) for the pure domain modules:
   - jar: any inserted cookie either returns for a matching URI or was
     correctly rejected by domain/path/secure match (RFC 6265bis §5.3/5.4
     as invariants); RFC worked examples stay as concrete cases too.
   - HeaderMap: case-insensitive lookup agrees with case-folded lookup;
     iteration preserves insertion order.
4. **Focused units** for error mapping and edge algorithms.

Rules adopted verbatim from code-conventions:

- The loopback server is a **recording fake**: tests assert against the
  method/path/headers it received — never spy/call-count assertions.
- **No test-only exports**: no `#[cfg(test)]` escapes into production
  code; every assertion walks the real public API.
- Nothing tested that the type system guarantees (getter/enum-shape
  tests, upstream `url` crate behavior).

Live coverage:

- Live smoke stays manual/opt-in (example.com dial).
- One peet.ws JA4 echo sanity check confirms v1 presents the expected
  OpenSSL fingerprint (drift detection, not gate-passing).
- **No live bot-gate matrix re-run at v1** — ADR 0006 owns the next full
  run at the stealth milestone; re-running now only re-measures the known
  9/16.
