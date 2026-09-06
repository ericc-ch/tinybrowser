# Reading engine source without cloning

Ground truth for how browsers behave is the WHATWG spec. Engine source
is the second source, used when the spec is silent or engines diverge.

Read in this order:

1. Spec.
2. Firefox (Gecko).
3. Chromium (Blink) if Gecko is silent or they disagree, or the question
   is Blink, V8-adjacent, or Chrome net.
4. WebKit if those two still disagree.

Search and fetch single files. Never clone these repos; they are far too
big. Chromium and WebKit are the same class of reference as Gecko, not
trees to keep locally.

## Firefox (current default)

- Search (Searchfox): [searchfox.org/mozilla-central](https://searchfox.org/mozilla-central/source/)
- Single-file fetch: `https://raw.githubusercontent.com/mozilla-firefox/firefox/<rev>/path`
- Browse: [github.com/mozilla-firefox/firefox](https://github.com/mozilla-firefox/firefox)

## Chromium / Blink

Official docs list code search and Gitiles as the source browsers
([chromium/src `docs/useful_urls.md`](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/useful_urls.md)).
Layout of the tree:
[Getting around the Chromium source](https://www.chromium.org/developers/how-tos/getting-around-the-chrome-source-code/).
Checkout size warning is in
[`docs/get_the_code.md`](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/get_the_code.md)
and the GitHub mirror README: do not `git clone`; use `fetch` only if
you are actually building Chrome.

For our work, DOM / HTML / CSS live under Blink:

`third_party/blink/renderer/` (especially `core/dom`, `core/html`,
`core/css`, `core/script`). Chrome chrome and `content/` are the
multi-process shell, not the page model.

| Use | URL |
| --- | --- |
| Search + xrefs (Searchfox analogue) | [source.chromium.org/chromium/chromium/src](https://source.chromium.org/chromium/chromium/src) |
| Older codesearch (same index family) | [cs.chromium.org](https://cs.chromium.org) |
| Gitiles tree | [chromium.googlesource.com/chromium/src](https://chromium.googlesource.com/chromium/src/) |
| GitHub mirror (plain files) | [github.com/chromium/chromium](https://github.com/chromium/chromium) |
| Single-file fetch | `https://raw.githubusercontent.com/chromium/chromium/<rev>/path` |

Example deep link:

`https://source.chromium.org/chromium/chromium/src/+/main:third_party/blink/renderer/core/dom/node.cc`

Gitiles `?format=TEXT` returns **base64**, not the file
([Gitiles API](https://gerrit.googlesource.com/gitiles/+/HEAD/Documentation/api-reference.md)).
Prefer the GitHub raw host. The GitHub mirror has gone stale before
([embedder-dev, 2026-02](https://groups.google.com/a/chromium.org/g/embedder-dev/c/lQYJwvzjdfo));
if a path 404s or looks old, open the same path on Gitiles / source.chromium.org.

Verified 2026-08-28: GitHub raw and Gitiles both 200 for
`third_party/blink/renderer/core/dom/node.cc` on `main` / `HEAD`.

## WebKit

Searchfox indexes WebKit as **wubkat** on the same host as Gecko.

For our work, DOM / HTML / CSS live under WebCore:

`Source/WebCore/` (especially `dom`, `html`, `css`, `bindings`).

| Use | URL |
| --- | --- |
| Search + xrefs | [searchfox.org/wubkat](https://searchfox.org/wubkat/source/) |
| GitHub | [github.com/WebKit/WebKit](https://github.com/WebKit/WebKit) |
| Single-file fetch | `https://raw.githubusercontent.com/WebKit/WebKit/<rev>/path` |

Verified 2026-08-28: GitHub raw 200 for `Source/WebCore/dom/Node.cpp` on `main`.
