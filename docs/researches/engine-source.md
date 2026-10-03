# Engine source

Reference links for reading shipped engine implementations. For Web API
implementation or binding changes, read [Web API bindings](../bindings.md).

Ground truth is the WHATWG spec. For how a browser behaves, read Chromium,
then Firefox if Chromium does not cover the case. Read WebKit if those two
still disagree. Search and fetch single files. Never clone these repositories.

## Fetch without cloning

### Chromium / Blink

Page model: `third_party/blink/renderer/` (`core/dom`, `core/html`,
`core/css`, `core/script`). The browser process and `content/` are the
multi-process shell, not the page model.

| Use | URL |
| --- | --- |
| Search | [source.chromium.org](https://source.chromium.org/chromium/chromium/src) |
| Gitiles | [chromium.googlesource.com/chromium/src](https://chromium.googlesource.com/chromium/src/) |
| Single-file fetch | `https://raw.githubusercontent.com/chromium/chromium/<rev>/path` |

Example:
`https://source.chromium.org/chromium/chromium/src/+/main:third_party/blink/renderer/core/dom/node.cc`

Gitiles `?format=TEXT` returns base64. Prefer the GitHub raw host. If a path
404s, open the same path on Gitiles.

### Firefox

| Use | URL |
| --- | --- |
| Search | [searchfox.org/mozilla-central](https://searchfox.org/mozilla-central/source/) |
| Single-file fetch | `https://raw.githubusercontent.com/mozilla-firefox/firefox/<rev>/path` |

### WebKit

DOM / HTML / CSS live under `Source/WebCore/` (`dom`, `html`, `css`,
`bindings`).

| Use | URL |
| --- | --- |
| Search | [searchfox.org/wubkat](https://searchfox.org/wubkat/source/) |
| Single-file fetch | `https://raw.githubusercontent.com/WebKit/WebKit/<rev>/path` |
