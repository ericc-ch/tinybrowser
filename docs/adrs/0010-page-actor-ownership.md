# Autonomous page actors and browser-owned resources

Status: accepted. Replaces the caller-driven pump, page-owned blocking pools,
post-parse script scan, and best-effort profile writes previously recorded here.

## Decision

One profile daemon hosts one `Browser`. `Browser` owns the page registry,
`ProfileStore`, shared cookie jar, and one bounded network executor. The executor
has 16 blocking workers and a 256-job browser-wide queue. A page submits
value-only work tagged by its navigation or JavaScript epoch. Queued work checks
page cancellation before starting; closing a page discards later completions.

Each `PageActor` owns its DOM, active HTML parser, QuickJS realm, timers, and
navigation state on one OS thread. It advances work while idle. Wait requests
register conditions and do not monopolize the actor. `PageHandle` crosses this
boundary using commands, request IDs, values, event receivers, and explicit
errors. DOM references, QuickJS values, and callbacks stay on the actor.

The public `tinybrowser` crate exposes `Browser`, `BrowserHandle`, and
`PageHandle`, not the directly driven engine `Page`. The lower-level `browser`
workspace crate keeps parser and direct-page APIs for conformance tests.

## Parsing and scripts

Navigation bytes are decoded before tokenization using BOM, HTTP charset, and
the HTML prescan, with Windows-1252 as the fallback. This follows the
[HTML encoding sniffing algorithm](https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm)
and the [Encoding label lookup](https://encoding.spec.whatwg.org/#concept-encoding-get).

The page drives html5ever incrementally. On a parser-blocking script end tag it
pauses tokenization, exposes the partial DOM to QuickJS, runs the script, applies
DOM mutations and parser-time `document.write()` input, then resumes the same
tokenizer. The relevant behavior is defined by HTML's
[text insertion mode](https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-intext),
[script preparation](https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element),
and [dynamic markup insertion](https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document-write-steps).

## Scheduling, events, and limits

The page loop multiplexes commands, timer deadlines, network completions,
QuickJS jobs, and wait conditions. `Page.enable` subscribes CDP clients to
unsolicited page events; navigation starts asynchronously and load completion is
reported as `Page.loadEventFired`.

Every QuickJS entry point has a five-second default execution budget, a 32 MiB
heap limit, a 512 KiB stack limit, and a stop-aware interrupt handler. An
explicit protocol deadline may shorten the execution budget. This makes page
shutdown independent of script cooperation.

## DOM bindings

DOM wrappers retain one native `NodeId` host representation, but public WebIDL
prototype objects expose only the members belonging to their interfaces.
`Document`, `Element`, `CharacterData`, `Text`, `Comment`, `DocumentType`, and
`DocumentFragment` inherit through `Node`, which inherits from `EventTarget`, according to the
[WebIDL interface prototype model](https://webidl.spec.whatwg.org/#interface-prototype-object).
`childNodes` and `getElementsByTagName()` return live `NodeList` and
`HTMLCollection` host objects rather than array snapshots.

## Persistence

Opening a durable profile takes an OS-backed exclusive file lock. A second
writer fails explicitly. Invalid cookie data is renamed to a timestamped
`cookies.corrupt.*` file and the profile starts with an empty jar. Other read
errors propagate. Writes use a same-directory temporary file, file `fsync`,
atomic rename, and directory `fsync`; failures remain dirty and propagate from
explicit `BrowserHandle::close`.

Shutdown first refuses new work, then closes every page, and finally persists
the quiescent cookie jar. Repeating close retries a failed durable write.

## Isolation

Threads enforce ownership but are not a security boundary. A later renderer
process may reuse the value-only page seam. Process isolation is not required to
make ordering, cancellation, resource bounds, and shutdown correct in-process.
