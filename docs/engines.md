# Engines

Ground truth is the WHATWG spec. For how a browser behaves, read Chromium,
then Firefox, then WebKit. Fetch single files; never clone.

| Engine | Search | Single file |
| --- | --- | --- |
| Chromium | source.chromium.org | `https://raw.githubusercontent.com/chromium/chromium/<rev>/path` |
| Firefox | searchfox.org/mozilla-central | `https://raw.githubusercontent.com/mozilla-firefox/firefox/<rev>/path` |
| WebKit | searchfox.org/wubkat | `https://raw.githubusercontent.com/WebKit/WebKit/<rev>/path` |

Blink page model: `third_party/blink/renderer/` (`core/dom`, `core/html`,
`core/css`). WebKit: `Source/WebCore/` (`dom`, `html`, `css`, `bindings`).
Gitiles `?format=TEXT` is base64; prefer the raw host.
