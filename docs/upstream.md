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
- 19672f4f tag requests with their resource kind (stylesheets, images, fonts). Per-destination policy (CSP `img-src` / `style-src` / `font-src`) cannot be applied from the URL alone; the embedder would have to guess the destination.
- 4736fc73 accept a leading `+` in canvas width/height attributes. The HTML non-negative integer parser allows it, so `width="+100"` is 100.
- 83251583 parse canvas width/height attributes as non-negative integers. `100%`, `100e1`, and `0x100` are 100, 100, and 0, not a CSS length and a parse failure; the intrinsic size and the computed width both depend on it.
- 82eb84a8 expose the document font context. Canvas text must shape with the database holding `@font-face` fonts; the shared process context does not see them, so an embedder cannot otherwise measure or draw the fonts a page declares.
- c198da78 accept zero-sized SVG images. usvg rejects width or height 0, so a valid image with no painted area was reported as a failed load; the declared intrinsic size now survives and `naturalWidth` / `naturalHeight` can be 0 without an error.

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
  - tinybrowser's release profile cannot honor this: `panic = "abort"` means std's `catch_unwind` never catches, so a Rust panic in a native callback aborts the renderer instead of raising a JS exception. Catching requires unwinding, measured at 13.59 MiB vs 10.61 MiB shipped (+2.98 MiB, `docs/size.md`), so it stays rejected; the panic hook records the panic with location (`crates/logging/src/crash.rs`) and callback paths must stay panic-free. The guard remains correct for unwind hosts (tests, embedders).
  - Deferred direction: redesign the fork so it never catches a panic to begin with (no `catch_unwind`, callbacks assumed panic-free), which makes the abort profile the documented contract instead of an exception to the guard.
- ad2fb33 expose the string compilation host hook. `Runtime::set_can_compile_strings` binds `JS_SetCanCompileStringsHandler` with the callback's `Ctx` and source, mirroring the interrupt and rejection handlers; bundle the new symbol for every target.
- Tracking (11c770e, 7ca6024, 38c4d87, a86ed40, 3587d7e, 50b403e, c161d25): pull quickjs engine fixes and regenerate bindings; wasm OOM/atom-table/inline mirrors.

### quickjs (third_party/rquickjs/sys/quickjs, fork branch)

Exotic fallthrough machinery the stock engine lacks (2afa33b, 88a9be3, 2339e92, 0ef07c2, 2c3319f, bd5bd06, cca3414, 68de392, c3297d6, 3c186c9). WebIDL collections must intercept indexed/named writes yet decline ordinary properties, inherited setters, and expandos back to the normal path, with strict-mode rejection and no duplicate keys. Each carries an rquickjs binding proof plus api-test coverage (f3b23e9).

- 712757e native closures retain and enter their home realm. C closures lacked the context ownership and GC edge C functions had; also adds length-zero stack checks and fallible init before opaque adoption.
- ac85002 keep the callback's exotic key order. The engine re-sorted host keys; now it copies the host order and appends only unreported ordinary keys.
- 12c216c consult the host before compiling strings from JS. `JS_SetCanCompileStringsHandler` runs at `JS_EvalObject`, the one choke point under `eval` (direct and indirect) and the `Function` constructor; host evaluation (`JS_Eval`) is untouched. An `eval`/`Function` JS wrapper cannot substitute: it is observable and makes every direct eval indirect.

### Vendored crates (no commits, recorded deltas)

- fontconfig-parser 0.5.8 with roxmltree 0.21.1 (upstream pins 0.20.0). usvg and fontique already pull 0.21.1, so the pin linked two XML parsers; one requirement bump dedups them. Upstream sources and tests unchanged.
- weedle4 at 7dd96ea plus an async_sequence terminal and type. WebIDL async-iterable support; the generator rejects conversions it cannot generate.

## Open issues

- Character data, titles, and attributes keep lone surrogates in a tinybrowser side table (`exact_text`, `exact_attrs`). The layout tree still stores lossy UTF-8, so style matching never sees a U+FFFD stand-in. Fixed on our side, not in the fork's storage.
- `blitz-dom` hosts shadow roots (`NodeData::ShadowRoot`, `ElementData::shadow_root`, blitz@ca29873). `attachShadow` returns the root, closed mode makes `shadowRoot` null, and style walks the shadow host without panicking. `ShadowRootInit` members other than `mode` (`delegatesFocus`, `clonable`, `serializable`, `slotAssignment`) are not stored. Shadow-scoped cascade data is still absent, so a `<style>` inside a shadow tree does not apply, and box construction still walks light-DOM children. Composed events do not cross a shadow root: the path stops there and `event.target` is not retargeted to the host. `getComposedRanges` stays a no-op. Partially fixed in fork.
- Reading `HTMLTemplateElement.content` copies the template contents into an inert document. `content.ownerDocument` is that document. Nested templates that have not been read stay same-document fragments inside it. Fixed on our side (fragment plumbing remains blitz@ffec5987/96443edf).
- XML namespace prefix synthesis on serialization is skipped (stored qualified names used as-is). Open, not fixed in fork. Entity half is fixed: internal general entities covered by blitz@c3581533 and html5ever@37c1cb0/ae2057f.
- `ElementData.form_state` stores dirty value, checkedness, selectedness, and indeterminate (blitz@48a1590f). `:checked` and `:indeterminate` read those flags. A textual `.value` sets the dirty value and leaves the content attribute alone; cloning carries the flags and setting `value` collapses the caret to the end. Fixed in fork. `select` events still have no producer.
- `blitz-dom` exposes no image-request state: `image_cache` / `pending_images` are `pub(crate)`, so request tracking (selected `currentSrc`, loading/broken flags, `load` / `error` events) stays ours; the decoded result itself is public (`ElementData::image_data`), so `naturalWidth` / `naturalHeight` and the `load` / `error` decision read Blitz state with no second decode. SVG and GIF `<img>` now load like Chromium. Open upstream gap, handled on our side.
- SVG `<image>` elements do not carry decoded image data after Blitz resolves their `href`; only HTML `<img>` gets `ElementData::image_data`. `createPattern` therefore cannot distinguish a missing resource from a valid zero-sized one for `<svg><image>`, and returns null for both, failing `2d.pattern.svgimage.nonexistent`. Open, not worked around.
- `DocumentConfig.quirks_mode` locks the stylist to the doctype sniff before parse (blitz@48a1590f), so html5ever's empty system id cannot override it. `document.compatMode` and layout share that mode. Fixed in fork. User-agent sheets stay NoQuirks.
- `blitz-html` sniffs XHTML by substring (`html_sink.rs:parse_into_mutator`: first line containing `XHTML`/`xhtml`, or an `xmlns` on the root) and parses the whole document as XML. Any `text/html` document whose first line mentions XHTML — e.g. an XHTML-transitional doctype — loses every node after the first well-formedness error. The HTML parser must never go XML from the doctype. Open, not fixed in fork, not worked around.
- `blitz-dom` puts `AnonymousBlock` layout boxes in the same tree. Open upstream gap, handled on our side as transparent (snapshot/serialize children only, never brand as elements).
- `blitz-dom` exposes only the viewport scroll offset, no per-element scroll-container offset API. Open, not fixed in fork.
- `blitz-dom` owns document language internally with no metadata setter, so a response `Content-Language` is not applied. Open, not fixed in fork.
- Document and fragment parses set `scripting_enabled` (blitz@48a1590f), so `noscript` is raw text. Fixed in fork.
- `blitz-dom` matches `:placeholder-shown` from `ElementState::PLACEHOLDER_SHOWN` (blitz@bcf0197). The bit follows the live control value: `editor.raw_text()` once a text editor exists, otherwise the value attribute or textarea text. An input event snapshots the bit, so typing clears it without writing the value attribute. Fixed in fork.
- `blitz-dom` inserts a document stylesheet before the tree-order successor of its owner, and `document.styleSheets` walks those owners in the same tree order (blitz@bcf0197), so moving the owner changes both cascade position and CSSOM order. Fixed in fork.
- `moveBefore` reparents through `DocumentMutator::move_node` (blitz@ca29873), which skips the insertion and removing steps and re-seats stylesheets already in the cascade. Fixed in fork.
- `tiny-skia` under-covers pixels on a canvas border when a path extends past the bitmap edge: an arc whose boundary lies just outside pixel (0,0) gets 239/255 coverage there, while the same path shifted one pixel inward gets 255. `2d.path.arc.scale.1` asserts those corner pixels, so the shape is right but the edge pixel is not. Open, not worked around.
- `hyper`'s HTTP/1 parser (`httparse`) rejects response header values containing control bytes other than TAB (0x0B, 0x0C, 0x7F, …), so the whole response fails before tinybrowser sees it. Chrome and Firefox accept such values; WPT relies on it (`mimesniff/mime-types/charset-parameter.window.js` heads six navigable fixtures with these bytes). Open, not worked around.
- `mimesniff/mime-types/parsing.any.js` drives the MIME parser through `Blob`/`File` `type`, but FileAPI defines that type as the raw string ASCII-lowercased (rejected only when a code unit falls outside U+0020..U+007E), so the two cannot both hold for inputs like `TEXT/HTML;CHARSET=GBK`. Both browsers implement FileAPI and fail the file (712/1898 Chrome, 1031/1898 Firefox at WPT 9f868fd7, while `FileAPI/blob/Blob-constructor.any.html` is 73/73 in both). The `Request`/`Response` `blob()` half does extract a MIME type and passes. Open, not worked around.
- Proposal tests fail everywhere by design: HTML `<?...?>` stays a bogus comment per the HTML Standard (the PI-attributes proposal expects PI nodes), and two PI value subtests (`axx>`, `some<>`) contradict the spec's own escaping algorithm. By design, not a bug.
