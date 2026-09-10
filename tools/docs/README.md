# docs toolchain

The architecture page is written in Markdown and rendered to HTML.

- `docs/architecture.md` — source of truth. Edit this.
- `tools/docs/template.html` — pandoc HTML template (theme, header, footer, Mermaid init).
- `tools/docs/mermaid.lua` — pandoc filter that emits Mermaid blocks as raw
  `<pre class="mermaid">` text. Pandoc's default wraps the code in `<code>`,
  and Mermaid's entity decode preserves that tag, which makes every diagram
  fail with “No diagram type detected”.
- `docs/architecture.html` — generated; never edit by hand.

Render:

```sh
./tools/docs/build.sh
```

Requirements: `pandoc` (3.x). If it is not on `PATH`:

```sh
nix shell nixpkgs#pandoc -c ./tools/docs/build.sh
```

Diagrams are Mermaid fenced blocks. The generated page imports Mermaid from
`cdn.jsdelivr.net` (pinned version) at load time, so viewing diagrams needs
network access once per browser cache. The Markdown itself always reads fine
without it.
