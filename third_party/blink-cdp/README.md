# Blink inspector-protocol tests

This directory is a vendored snapshot of Chromium's two inspector-protocol
web-test trees. It is test input and is not included in the tinybrowser binary.

- Revision: see `REVISION`
- Upstream: <https://chromium.googlesource.com/chromium/src/>
- `inspector-protocol/` comes from
  `https://chromium.googlesource.com/chromium/src/+archive/$REVISION/third_party/blink/web_tests/inspector-protocol.tar.gz`
- `http-inspector-protocol/` comes from
  `https://chromium.googlesource.com/chromium/src/+archive/$REVISION/third_party/blink/web_tests/http/tests/inspector-protocol.tar.gz`
- `LICENSE` is Chromium's license at the same revision
- Update: set `REVISION`, update the pin recorded in `../../third_party/VENDORED.md`,
  then run `./tools/cdp/update`

Do not edit vendored tests or expected output. Runner-specific policy belongs
under `tools/cdp/`.
