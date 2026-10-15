# Upstream bugs

Policy: upstream bugs stay upstream. Our code stays simple; these fail closed as known-fails.

## Fork changes

### blitz (third_party/blitz, master)

- c62d5c63 resolve_url returns None instead of panicking. A relative ref against a non-base (observed: foo.jpg against data:text/css) panicked the engine; unresolvable URLs now skip the fetch and form submission returns early.
- 7189584d real doctype, processing-instruction, and CDATA nodes. These are real DOM nodes; a tree that cannot represent them forces every embedder to rebuild them from source with side tables.
- 4954fea7 keep PI data, doctype nodes; drop only the XML declaration. The declaration is prolog, not content, so it creates no node; a later reserved-target PI stays a real node.
- ffec5987 template contents fragments and Fragment nodes. ElementData already carried a template_contents slot nothing filled, and deep clone shared the original; now templates are born with contents and clone/move preserve them.
- 96443edf host-linked fragments; template fragment parsing lands in contents. Cycle checks must see through fragments, and html5ever never opens the context element so its template modes have nowhere to route into.
- 5f6172ed carry sink parse errors on the document for draining. HTML ignores them (error-tolerant) while XML consumers decide parsererror documents from them.
- a6906d6b fail closed on deep entity nesting, scan only the prolog for DOCTYPE, record one error. Deep nesting used to stop silently and leave a well-formed document (billion-laughs hole); a doctype keyword inside comments/PIs declared entities; 10k bad references produced 10k strings.
- c3581533 let xml5ever expand internal general entities. The 797-line bounds module belongs to the parser crate that owns the DTD subset.
- 791d7655 drop the on-demand style context inside the layout state. SequentialTaskList::drop asserts the state, so getComputedStyle on display:none panicked as soon as the context dropped.
- 9f10f12e display:contents generates no box for resolved values. Per CSSOM, no box means the resolved value is the computed value; it reported 0px and leaked min/max clamping in.

### html5ever (third_party/html5ever, master)

- 1753ef5 keep xmlns attributes, CDATA nodes, strict EOF/doctype. DOM trees need what out-of-band-namespace embedders do not; restores the essence of upstream #586 (reverted upstream in #595). EOF-with-open-elements and PUBLIC-without-SYSTEM are well-formedness violations that must surface as parsererror.
- b71b746 skip the internal DTD subset instead of going bogus. The subset carries no tree data (expansion runs up front) but its presence must not poison the doctype token.
- 39ba978 / bac928b prolog-only documents, then EOF-before-root is an error. First half accepted rootless input, then the start-phase half was reverted: the XML document production requires exactly one root, as Gecko and Chromium enforce. Unclosed roots are still caught in the end phase.
- 37c1cb0 expand internal general entities in the parser. Moved from blitz-html source pre-expansion; the driver owns it so markup inside replacement text parses and errors reach the sink.
- ae2057f fail closed on budget exhaustion and keep literals intact. Oversized replacements were half-copied, and expanded text containing the attribute delimiter could re-delimit the value or start a new reference.
- 08c7e33 drop the unused TreeSink import. Warning cleanup.
- merged upstream main at 7760920 (0.40.1).

### rquickjs (third_party/rquickjs, master)

Exotic hooks for WebIDL legacy platform objects (dcd8edd, 9bcc543, 42688aa, 31e1683, 62d9c1a, 09be401, f99e205, 52744cf, cfc6c77, de4b9d9). The shared QuickJS class installed get/has/set callbacks for every exotic object, hiding prototypes and swallowing ordinary assignment. Now each Rust type registers only its derived hooks with explicit fallthrough to the ordinary path, so OwnPropertyKeys ordering, IsDataDescriptor, and receiver/holder distinction match WebIDL. Each engine-side fix has a binding proof that was red before it.

- d73fbda safe realm-aware native callbacks with copyable dispatch state. Callbacks keep argument counts, receive new.target, and run in their creation realm; Copy/Send/Sync-only state fits GC (no Rust destructors mid-collection).
- 8842327 UTF-16 string conversion preserving lone surrogates. WebIDL DOMString conversion keeps every code unit; pairs with the blitz UTF-8 open issue below (JS side preserves, DOM side does not).
- 103de1e survive caught panics and regenerated binding types. A panic caught across FFI poisoned the mutex and broke every later call; plus an MSVC atom-type build fix and per-target test gating.
- Tracking (11c770e, 7ca6024, 38c4d87, a86ed40, 3587d7e, 50b403e, c161d25): pull quickjs engine fixes and regenerate bindings; wasm OOM/atom-table/inline mirrors.

### quickjs (third_party/rquickjs/sys/quickjs, fork branch)

Exotic fallthrough machinery the stock engine lacks (2afa33b, 88a9be3, 2339e92, 0ef07c2, 2c3319f, bd5bd06, cca3414, 68de392, c3297d6, 3c186c9). WebIDL collections must intercept indexed/named writes yet decline ordinary properties, inherited setters, and expandos back to the normal path, with strict-mode rejection and no duplicate keys. Each carries an rquickjs binding proof plus api-test coverage (f3b23e9).

- 712757e native closures retain and enter their home realm. C closures lacked the context ownership and GC edge C functions had; also adds length-zero stack checks and fallible init before opaque adoption.
- ac85002 keep the callback's exotic key order. The engine re-sorted host keys; now it copies the host order and appends only unreported ordinary keys.

### Vendored crates (no commits, recorded deltas)

- fontconfig-parser 0.5.8 with roxmltree 0.21.1 (upstream pins 0.20.0). usvg and fontique already pull 0.21.1, so the pin linked two XML parsers; one requirement bump dedups them. Upstream sources and tests unchanged.
- weedle4 at 7dd96ea plus an async_sequence terminal and type. WebIDL async-iterable support; the generator rejects conversions it cannot generate.

## Open issues

- `blitz-dom` stores text as UTF-8, so lone surrogates in `CharacterData`, attributes, and titles read back as U+FFFD. No side table preserves them. Open, not fixed in fork.
- `blitz-dom` has no shadow DOM: no shadow roots, `attachShadow` never hosts, `ShadowRoot` brand never instantiates, `getComposedRanges` / composed options are no-ops. Open, not fixed in fork.
- Template contents share their host document instead of a separate inert template-contents owner document, so `content.ownerDocument !== document` assertions fail. Needs per-document inert owner documents with cross-arena contents routing; deferred as its own feature. Open, not fixed in fork (fragment plumbing fixed in blitz@ffec5987/96443edf, owner-doc split still open).
- XML namespace prefix synthesis on serialization is skipped (stored qualified names used as-is). Open, not fixed in fork. Entity half is fixed: internal general entities covered by blitz@c3581533 and html5ever@37c1cb0/ae2057f.
- `blitz-dom` keeps no form-control state: no dirty value flag, checkedness, selectedness, or indeterminate slots; values read from content attributes and descendant text, `select` events have no producer, form-state cloning carries structure only. Open, not fixed in fork.
- `blitz-dom` exposes no image-request state: `image_cache` / `pending_images` are `pub(crate)`, so request tracking (selected `currentSrc`, loading/broken flags, `load` / `error` events) stays ours; the decoded result itself is public (`ElementData::image_data`), so `naturalWidth` / `naturalHeight` and the `load` / `error` decision read Blitz state with no second decode. SVG and GIF `<img>` now load like Chromium. Open upstream gap, handled on our side.
- `blitz-dom` styles with hardcoded `NoQuirks` internally, so `document.compatMode` (sniffed from the doctype) can disagree with the rendering mode. Open, not fixed in fork.
- `blitz-html` sniffs XHTML by substring (`html_sink.rs:parse_into_mutator`: first line containing `XHTML`/`xhtml`, or an `xmlns` on the root) and parses the whole document as XML. Any `text/html` document whose first line mentions XHTML — e.g. an XHTML-transitional doctype — loses every node after the first well-formedness error. The HTML parser must never go XML from the doctype. Open, not fixed in fork, not worked around.
- `blitz-dom` puts `AnonymousBlock` layout boxes in the same tree. Open upstream gap, handled on our side as transparent (snapshot/serialize children only, never brand as elements).
- `blitz-dom` exposes only the viewport scroll offset, no per-element scroll-container offset API. Open, not fixed in fork.
- `blitz-dom` owns document language internally with no metadata setter, so a response `Content-Language` is not applied. Open, not fixed in fork.
- `blitz-html` always parses scripting-disabled (`noscript` as markup); scripting-enabled `noscript`-as-text diverges. Open, not fixed in fork.
- `blitz-dom` `NonTSPseudoClass::PlaceholderShown` is hardcoded `false`, so `:placeholder-shown` never matches and placeholder text does not paint (observed: Wikipedia's "Search Wikipedia"). Open, not fixed in fork.
- `blitz-dom` orders a document's stylesheets by owner node id (the stylist insertion point is the next sheet's node id), so moving a `<link rel=stylesheet>` with `moveBefore` does not change its cascade position. Sheet order must follow the owner node's document position. Open, not fixed in fork.
- `blitz-dom`'s mutator has no move primitive, so `moveBefore` unlinks and re-inserts through the remove/insert paths: a moved `<link rel=stylesheet>` is unloaded and re-fetched, though the DOM `move` algorithm must not run insertion or removing steps. Needs a mutator move primitive that skips those special ops. Open, not fixed in fork.
- Proposal tests fail everywhere by design: HTML `<?...?>` stays a bogus comment per the HTML Standard (the PI-attributes proposal expects PI nodes), and two PI value subtests (`axx>`, `some<>`) contradict the spec's own escaping algorithm. By design, not a bug.
