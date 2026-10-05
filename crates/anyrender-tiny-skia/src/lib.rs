//! A [`tiny_skia`] backend for the [`anyrender`] 2D drawing abstraction.
//!
//! Immediate-mode painter: every scene command draws straight into the
//! current target pixmap. Layers render into transparent child pixmaps that
//! composite back on pop; clips accumulate as [`tiny_skia::Mask`]s. Text goes
//! through `skrifa` outlines, the same way the pre-Blitz in-tree painter did.

mod image_renderer;
mod scene;

pub use image_renderer::TinySkiaImageRenderer;
pub use scene::TinySkiaScenePainter;
