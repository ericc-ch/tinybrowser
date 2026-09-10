#!/usr/bin/env bash
# Render docs/architecture.md to docs/architecture.html with pandoc.
#
# The Markdown is the source of truth; the HTML is generated. Diagrams are
# Mermaid blocks rendered in the browser from a pinned CDN import.
set -euo pipefail

here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$here/../.." && pwd)"
src="$root/docs/architecture.md"
out="$root/docs/architecture.html"

if ! command -v pandoc >/dev/null 2>&1; then
  echo "pandoc not found. Install it, or run: nix shell nixpkgs#pandoc -c $0" >&2
  exit 1
fi

pandoc "$src" \
  --from=gfm+fenced_divs \
  --to=html5 \
  --standalone \
  --no-highlight \
  --lua-filter="$here/mermaid.lua" \
  --template="$here/template.html" \
  --metadata "title=tinybrowser — architecture, end to end" \
  --output="$out"

# Mermaid reads element.innerHTML and tags do not survive its entity decode;
# a <code> wrapper makes every diagram fail with "No diagram type detected".
if grep -q 'class="mermaid"><code' "$out"; then
  echo "error: mermaid blocks were wrapped in <code>; the Lua filter did not run" >&2
  exit 1
fi

echo "wrote $out"
