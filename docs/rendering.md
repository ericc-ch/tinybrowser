# Rendering

One-shot screenshots through Blitz. No compositor, no relayout.

```text
Blitz parse/style/layout -> blitz-paint scene -> anyrender_tiny_skia -> PNG
```

Blitz (`blitz-dom`/`blitz-html`/`blitz-paint`) owns parsing, style, layout,
and display-list construction. `crates/anyrender-tiny-skia` implements
`anyrender`'s `PaintScene` over `tiny-skia`. Screenshots flatten over white
in `crates/renderer/src/render/blitz.rs`.
