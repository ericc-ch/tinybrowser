//! CPU paint: solid rects, borders, and shaped text into one `tiny-skia`
//! pixmap.
//!
//! Layout produces paint-order rectangles and positioned glyph runs; this
//! module turns them into pixels once. There is no retained display list and
//! no compositor — the screenshot pipeline is one pass, like `NetSurf`'s
//! `nsfb` and Dillo's canvas backends.
//!
//! Glyphs arrive shaped and positioned from Parley; their outlines come from
//! `skrifa` and fill through `tiny-skia` with anti-aliasing. Text that
//! crosses an `overflow: hidden` clip edge can overpaint by the overlap:
//! glyphs fully outside the clip are skipped, partially overlapping ones are
//! not masked (masking every text leaf would allocate per leaf).

use tiny_skia::{
    Color as SkiaColor, FillRule, Paint, PathBuilder, Pixmap, Rect as SkiaRect, Transform,
};

use skrifa::GlyphId;
use skrifa::MetadataProvider as _;
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::OutlinePen;

use crate::color::Color;
use crate::geometry::Rect;
use crate::text::PlacedGlyph;
use crate::{RenderError, RgbaImage};

/// Output cap in device pixels; a 4x-scaled 800x600 viewport is well under
/// this, and the cap keeps a hostile viewport from allocating unbounded
/// memory before paint starts.
const MAX_PIXELS: u64 = 4096 * 4096;

/// Blends text glyphs and fills rectangles into one pixmap.
pub(crate) struct Painter {
    pixmap: Pixmap,
    /// Active clip rectangles, innermost last; empty means the whole canvas.
    clips: Vec<Rect>,
}

impl Painter {
    /// Creates a `width` x `height` painter, cleared to white.
    ///
    /// # Errors
    ///
    /// [`RenderError::TooLarge`] when the viewport exceeds the pixel cap.
    pub(crate) fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(RenderError::TooLarge);
        }
        let mut pixmap = Pixmap::new(width, height).ok_or(RenderError::TooLarge)?;
        pixmap.fill(SkiaColor::WHITE);
        Ok(Self {
            pixmap,
            clips: Vec::new(),
        })
    }

    /// Intersects the current clip with `rect`.
    pub(crate) fn push_clip(&mut self, rect: Rect) {
        let clip = match self.clips.last() {
            Some(current) => current.intersect(rect),
            None => rect,
        };
        self.clips.push(clip);
    }

    /// Restores the clip to the enclosing one.
    pub(crate) fn pop_clip(&mut self) {
        self.clips.pop();
    }

    /// The effective clip: the innermost push, or `None` for the canvas.
    fn current_clip(&self) -> Option<Rect> {
        self.clips.last().copied()
    }

    /// Fills a solid rectangle.
    pub(crate) fn fill_rect(&mut self, rect: Rect, color: Color) {
        let Some(visible) = self.visible(rect) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(color.r, color.g, color.b, color.a));
        if let Some(skia) = SkiaRect::from_xywh(visible.x, visible.y, visible.width, visible.height)
        {
            self.pixmap
                .fill_rect(skia, &paint, Transform::identity(), None);
        }
    }

    /// Draws shaped glyphs positioned by Parley.
    ///
    /// Each glyph outline comes from `skrifa` at the run size and fills
    /// through `tiny-skia`. Glyphs fully outside the clip are skipped.
    pub(crate) fn draw_glyphs(
        &mut self,
        glyphs: &[PlacedGlyph],
        face: &skrifa::FontRef<'_>,
        size: f32,
        color: Color,
    ) {
        if glyphs.is_empty() {
            return;
        }
        let outlines = face.outline_glyphs();
        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(color.r, color.g, color.b, color.a));
        paint.anti_alias = true;
        for glyph in glyphs {
            if !self.glyph_visible(glyph) {
                continue;
            }
            let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                continue;
            };
            let coords: &[NormalizedCoord] = &[];
            let settings =
                skrifa::outline::DrawSettings::unhinted(Size::new(size), LocationRef::from(coords));
            let mut pen = PathPen::new(glyph.x, glyph.y);
            if outline.draw(settings, &mut pen).is_err() {
                continue;
            }
            let Some(path) = pen.finish() else {
                continue;
            };
            self.pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }

    /// Whether any part of a glyph run entry can paint inside the clip.
    fn glyph_visible(&self, glyph: &PlacedGlyph) -> bool {
        let Some(clip) = self.current_clip() else {
            return true;
        };
        // Glyph extents are unknown before outlining; test the pen point
        // against the clip expanded by one em in every direction.
        glyph.x > clip.x - 64.0
            && glyph.x < clip.right() + 64.0
            && glyph.y > clip.y - 64.0
            && glyph.y < clip.bottom() + 64.0
    }

    /// The painted image, consuming the painter.
    pub(crate) fn into_image(self) -> RgbaImage {
        RgbaImage {
            width: self.pixmap.width(),
            height: self.pixmap.height(),
            data: self.pixmap.take(),
        }
    }

    /// `rect` clipped to the current clip and canvas, or `None` when nothing
    /// would be painted.
    fn visible(&self, rect: Rect) -> Option<Rect> {
        let rect = match self.current_clip() {
            Some(clip) => rect.intersect(clip),
            None => rect,
        };
        let canvas = Rect::new(
            0.0,
            0.0,
            crate::pixels(self.pixmap.width()),
            crate::pixels(self.pixmap.height()),
        );
        let rect = rect.intersect(canvas);
        if rect.is_empty() { None } else { Some(rect) }
    }
}

/// Collects one glyph outline into a `tiny-skia` path, flipping font
/// y-up coordinates to screen y-down around the pen position.
struct PathPen {
    /// Finished path under construction.
    builder: PathBuilder,
    /// Pen x in device pixels.
    x: f32,
    /// Baseline y in device pixels.
    y: f32,
}

impl PathPen {
    /// A pen drawing at `(x, y)`.
    fn new(x: f32, y: f32) -> Self {
        Self {
            builder: PathBuilder::new(),
            x,
            y,
        }
    }

    /// Finishes the path, or `None` when nothing was drawn.
    fn finish(self) -> Option<tiny_skia::Path> {
        self.builder.finish()
    }
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(self.x + x, self.y - y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(self.x + x, self.y - y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder
            .quad_to(self.x + cx0, self.y - cy0, self.x + x, self.y - y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            self.x + cx0,
            self.y - cy0,
            self.x + cx1,
            self.y - cy1,
            self.x + x,
            self.y - y,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}
