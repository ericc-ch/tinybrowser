# Rendering

One-shot screenshots: style, layout, paint, PNG. No compositor, no relayout.

```text
DOM + sheets -> Stylo cascade -> Style -> box tree
  -> Taffy layout (Parley measures inline) -> tiny-skia paint -> PNG
```

Stylo owns selectors, inheritance, computed values. Taffy owns box layout.
Parley owns shaping and line breaking. Paint is in-tree `tiny-skia`.

Subset `Style` keeps: solid `background-color` only (no image/gradient),
tables as `Block`, `sticky` as `Static`, no `calc()` symbolic form.
Paint draws rects, borders, images, SVG paths, glyphs.
