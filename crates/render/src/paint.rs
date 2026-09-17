//! CPU paint: solid rects, borders, and text into one `tiny-skia` pixmap.
//!
//! Layout produces paint-order rectangles and text runs; this module turns
//! them into pixels once. There is no retained display list and no
//! compositor — the screenshot pipeline is one pass, like `NetSurf`'s `nsfb`
//! and Dillo's canvas backends.
//!
//! Coverage for text comes from `fontdue`'s glyph bitmaps; they are blended
//! into the pixmap manually. `tiny-skia` pixmaps are premultiplied RGBA, and
//! the manual blend follows premultiplied source-over
//! (<https://drafts.csswg.org/css-color-4/#interpolation-alpha>).

use tiny_skia::{Color as SkiaColor, Paint, Pixmap, Rect as SkiaRect, Transform};

use crate::color::Color;
use crate::font::Fonts;
use crate::geometry::Rect;
use crate::text::FontStyle;
use crate::{RenderError, RgbaImage};

/// Output cap in device pixels; a 4x-scaled 800x600 viewport is well under
/// this, and the cap keeps a hostile viewport from allocating unbounded
/// memory before paint starts.
const MAX_PIXELS: u64 = 4096 * 4096;

/// Blends text glyphs and fills rectangles into one pixmap.
pub(crate) struct Painter {
    pixmap: Pixmap,
    fonts: Fonts,
    clip: Option<Rect>,
}

impl Painter {
    /// Creates a `width` x `height` painter, cleared to white, drawing with
    /// `fonts`.
    ///
    /// # Errors
    ///
    /// [`RenderError::TooLarge`] when the viewport exceeds the pixel cap.
    pub(crate) fn new(width: u32, height: u32, fonts: Fonts) -> Result<Self, RenderError> {
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(RenderError::TooLarge);
        }
        let mut pixmap = Pixmap::new(width, height).ok_or(RenderError::TooLarge)?;
        pixmap.fill(SkiaColor::WHITE);
        Ok(Self {
            pixmap,
            fonts,
            clip: None,
        })
    }

    /// Intersects the current clip with `rect`.
    pub(crate) fn push_clip(&mut self, rect: Rect) {
        self.clip = Some(match self.clip {
            Some(current) => current.intersect(rect),
            None => rect,
        });
    }

    /// Restores the clip to the whole canvas.
    pub(crate) fn pop_clip(&mut self) {
        self.clip = None;
    }

    /// Fills a solid rectangle.
    pub(crate) fn fill_rect(&mut self, rect: Rect, color: Color) {
        let Some(visible) = self.visible(rect) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_color(SkiaColor::from_rgba8(
            color.r, color.g, color.b, color.a,
        ));
        if let Some(skia) = SkiaRect::from_xywh(visible.x, visible.y, visible.width, visible.height)
        {
            self.pixmap
                .fill_rect(skia, &paint, Transform::identity(), None);
        }
    }

    /// Draws one line of text with `baseline` as the alphabetic baseline.
    ///
    /// The caller owns line breaking and whitespace processing; this draws
    /// exactly the characters it is given.
    pub(crate) fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        style: FontStyle,
        color: Color,
    ) {
        let source = color.premultiplied();
        let mut pen = x;
        for ch in text.chars() {
            let (metrics, bitmap) = self.fonts.rasterize(ch, style);
            let advance = metrics.advance_width;
            // Glyph bitmap origin: `xmin` right of the pen, and the top edge
            // sits `ymin + height` above the baseline.
            let left = pen + metric(metrics.xmin);
            let top = baseline - metric(metrics.ymin + i32::try_from(metrics.height).unwrap_or(i32::MAX));
            self.blend_glyph(&bitmap, metrics.width, metrics.height, left, top, source);
            pen += advance;
        }
    }

    /// Blends one glyph bitmap at pixel position `(left, top)`.
    fn blend_glyph(
        &mut self,
        bitmap: &[u8],
        width: usize,
        height: usize,
        left: f32,
        top: f32,
        source: [u8; 4],
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let left = rounded(left);
        let top = rounded(top);
        let canvas_width = i32::try_from(self.pixmap.width()).unwrap_or(i32::MAX);
        let canvas_height = i32::try_from(self.pixmap.height()).unwrap_or(i32::MAX);
        let (clip_left, clip_top, clip_right, clip_bottom) = match self.clip {
            Some(clip) => (
                rounded(clip.x.floor()),
                rounded(clip.y.floor()),
                rounded(clip.right().ceil()),
                rounded(clip.bottom().ceil()),
            ),
            None => (0, 0, canvas_width, canvas_height),
        };
        let stride = self.pixmap.width() as usize;
        let data = self.pixmap.data_mut();
        for row in 0..height {
            let y = top + i32::try_from(row).unwrap_or(i32::MAX);
            if y < clip_top.max(0) || y >= clip_bottom.min(canvas_height) {
                continue;
            }
            let Some(y) = usize::try_from(y).ok() else {
                continue;
            };
            for col in 0..width {
                let x = left + i32::try_from(col).unwrap_or(i32::MAX);
                if x < clip_left.max(0) || x >= clip_right.min(canvas_width) {
                    continue;
                }
                let Some(x) = usize::try_from(x).ok() else {
                    continue;
                };
                let coverage = u32::from(bitmap[row * width + col]);
                if coverage == 0 {
                    continue;
                }
                let index = (y * stride + x) * 4;
                for channel in 0..4 {
                    let existing = u32::from(data[index + channel]);
                    let blended =
                        (u32::from(source[channel]) * coverage + existing * (255 - coverage) + 127)
                            / 255;
                    data[index + channel] = u8::try_from(blended.min(255)).unwrap_or(255);
                }
            }
        }
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
        let rect = match self.clip {
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

/// Converts an integer font metric to pixels; glyph metrics are small.
#[expect(
    clippy::cast_precision_loss,
    reason = "fontdue metrics are within i32 but represent pixel positions far below 2^23"
)]
fn metric(value: i32) -> f32 {
    value as f32
}

/// Rounds a layout coordinate to its pixel row or column.
#[expect(
    clippy::cast_possible_truncation,
    reason = "canvas coordinates are bounded by MAX_PIXELS before this call"
)]
fn rounded(value: f32) -> i32 {
    value as i32
}
