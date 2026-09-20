//! CPU paint: solid rects, borders, and shaped text into one `tiny-skia`
//! pixmap.
//!
//! Layout produces paint-order rectangles and positioned glyph runs; this
//! module turns them into pixels once. There is no retained display list and
//! no compositor — the screenshot pipeline is one pass, like `NetSurf`'s
//! `nsfb` and Dillo's canvas backends.
//!
//! Glyphs arrive shaped and positioned from Parley; their outlines come from
//! `skrifa` and fill through `tiny-skia` with anti-aliasing. `overflow:
//! hidden` clips paint through one `tiny-skia` mask, including glyphs.

use tiny_skia::{
    Color as SkiaColor, FillRule, FilterQuality, Mask, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, PixmapRef, Rect as SkiaRect, Stroke, Transform,
};

use skrifa::GlyphId;
use skrifa::MetadataProvider as _;
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::OutlinePen;

use crate::render::color::Color;
use crate::render::geometry::Rect;
use crate::render::text::PlacedGlyph;
use crate::render::{RasterImage, RenderError, RgbaImage};

/// Output cap in device pixels; a 4x-scaled 800x600 viewport is well under
/// this, and the cap keeps a hostile viewport from allocating unbounded
/// memory before paint starts.
const MAX_PIXELS: u64 = 4096 * 4096;

/// Blends text glyphs and fills rectangles into one pixmap.
pub(crate) struct Painter {
    pixmap: Pixmap,
    /// Active clip rectangles, innermost last; empty means the whole canvas.
    clips: Vec<(Rect, [f32; 4])>,
    /// Raster clip matching [`clips`], rebuilt on push/pop.
    mask: Option<Mask>,
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
            mask: None,
        })
    }

    /// Intersects the current clip with `rect`, rounded by `radii`.
    pub(crate) fn push_clip(&mut self, rect: Rect, radii: [f32; 4]) {
        let clip = match self.clips.last() {
            Some((current, _)) => current.intersect(rect),
            None => rect,
        };
        self.clips.push((clip, radii));
        self.rebuild_mask();
    }

    /// Restores the clip to the enclosing one.
    pub(crate) fn pop_clip(&mut self) {
        self.clips.pop();
        self.rebuild_mask();
    }

    /// The effective clip: the innermost push, or `None` for the canvas.
    fn current_clip(&self) -> Option<Rect> {
        self.clips.last().map(|(clip, _)| *clip)
    }

    fn rebuild_mask(&mut self) {
        self.mask = None;
        let width = self.pixmap.width();
        let height = self.pixmap.height();
        let mut mask: Option<Mask> = None;
        for (clip, radii) in &self.clips {
            let Some(path) = clip_path(*clip, *radii) else {
                continue;
            };
            if let Some(built) = mask.as_mut() {
                built.intersect_path(&path, FillRule::Winding, true, Transform::identity());
            } else {
                let Some(mut built) = Mask::new(width, height) else {
                    return;
                };
                built.fill_path(&path, FillRule::Winding, true, Transform::identity());
                mask = Some(built);
            }
        }
        self.mask = mask;
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
                .fill_rect(skia, &paint, Transform::identity(), self.mask.as_ref());
        }
    }

    /// Fills a rectangle with independent circular corner radii.
    pub(crate) fn fill_rounded_rect(&mut self, rect: Rect, radii: [f32; 4], color: Color) {
        if self.visible(rect).is_none() {
            return;
        }
        let Some(path) = rounded_rect_path(rect, radii) else {
            return;
        };
        self.fill_vector_path(&path, color, Transform::identity());
    }

    /// Strokes a rounded rectangle for a uniform border.
    pub(crate) fn stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radii: [f32; 4],
        width: f32,
        color: Color,
    ) {
        if width <= 0.0 || self.visible(rect).is_none() {
            return;
        }
        let Some(path) = rounded_rect_path(rect, radii) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(color.r, color.g, color.b, color.a));
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        self.pixmap.stroke_path(
            &path,
            &paint,
            &stroke,
            Transform::identity(),
            self.mask.as_ref(),
        );
    }

    /// Scales one premultiplied RGBA image into its CSS content box.
    pub(crate) fn draw_image(&mut self, rect: Rect, image: &RasterImage) {
        if rect.is_empty() || self.visible(rect).is_none() {
            return;
        }
        let Some(source) = PixmapRef::from_bytes(&image.data, image.width, image.height) else {
            return;
        };
        let paint = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        let transform = Transform::from_row(
            rect.width / crate::render::pixels(image.width),
            0.0,
            0.0,
            rect.height / crate::render::pixels(image.height),
            rect.x,
            rect.y,
        );
        self.pixmap
            .draw_pixmap(0, 0, source, &paint, transform, self.mask.as_ref());
    }

    /// Fills one vector path after mapping its SVG user coordinates.
    pub(crate) fn fill_vector_path(
        &mut self,
        path: &tiny_skia::Path,
        color: Color,
        transform: Transform,
    ) {
        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(color.r, color.g, color.b, color.a));
        paint.anti_alias = true;
        self.pixmap.fill_path(
            path,
            &paint,
            FillRule::Winding,
            transform,
            self.mask.as_ref(),
        );
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
                self.mask.as_ref(),
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
            crate::render::pixels(self.pixmap.width()),
            crate::render::pixels(self.pixmap.height()),
        );
        let rect = rect.intersect(canvas);
        if rect.is_empty() { None } else { Some(rect) }
    }
}

/// Clip path for `overflow: hidden`: a rectangle, or a rounded rectangle
/// when `border-radius` applies
/// (<https://drafts.csswg.org/css-overflow-3/#overflow-clip>).
fn clip_path(rect: Rect, radii: [f32; 4]) -> Option<Path> {
    if radii.iter().any(|radius| *radius > 0.0) {
        rounded_rect_path(rect, radii)
    } else {
        SkiaRect::from_xywh(rect.x, rect.y, rect.width.max(0.0), rect.height.max(0.0))
            .map(PathBuilder::from_rect)
    }
}

/// Builds a clockwise rounded rectangle. CSS scales overlarge radii down so
/// adjacent corners never overlap.
fn rounded_rect_path(rect: Rect, mut radii: [f32; 4]) -> Option<Path> {
    if rect.is_empty() {
        return None;
    }
    let max = rect.width.min(rect.height) / 2.0;
    for radius in &mut radii {
        *radius = radius.clamp(0.0, max);
    }
    let [top_left, top_right, bottom_right, bottom_left] = radii;
    let mut path = PathBuilder::new();
    path.move_to(rect.x + top_left, rect.y);
    path.line_to(rect.right() - top_right, rect.y);
    path.quad_to(rect.right(), rect.y, rect.right(), rect.y + top_right);
    path.line_to(rect.right(), rect.bottom() - bottom_right);
    path.quad_to(
        rect.right(),
        rect.bottom(),
        rect.right() - bottom_right,
        rect.bottom(),
    );
    path.line_to(rect.x + bottom_left, rect.bottom());
    path.quad_to(rect.x, rect.bottom(), rect.x, rect.bottom() - bottom_left);
    path.line_to(rect.x, rect.y + top_left);
    path.quad_to(rect.x, rect.y, rect.x + top_left, rect.y);
    path.close();
    path.finish()
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
