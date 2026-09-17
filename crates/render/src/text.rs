//! Text measurement and glyph raster access over [`Fonts`].
//!
//! Line breaking and whitespace processing live in `layout.rs`; this module
//! only answers questions the layout needs: how wide is a run, how tall is a
//! line, and what bitmap covers a glyph.

use fontdue::Metrics;

use crate::font::{Fonts, Weight};

/// The font properties a layout pass needs from one computed style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FontStyle {
    /// `font-size` in CSS pixels.
    pub size: f32,
    /// Resolved `font-weight`.
    pub weight: Weight,
}

/// Vertical metrics of one line for a face
/// (<https://drafts.csswg.org/css2/#leading>).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LineMetrics {
    /// Distance above the baseline.
    pub ascent: f32,
    /// Distance below the baseline.
    pub descent: f32,
    /// Extra leading the face reports between lines.
    pub line_gap: f32,
}

impl Fonts {
    /// The face's line metrics at `style`'s size.
    ///
    /// Faces without `hhea` line metrics fall back to the common 0.8/0.2
    /// split of the em box; the embedded faces always report.
    pub(crate) fn line_metrics(&self, style: FontStyle) -> LineMetrics {
        let face = self.face(style.weight);
        match face.horizontal_line_metrics(style.size) {
            Some(metrics) => LineMetrics {
                ascent: metrics.ascent,
                descent: -metrics.descent,
                line_gap: metrics.line_gap,
            },
            None => LineMetrics {
                ascent: style.size * 0.8,
                descent: style.size * 0.2,
                line_gap: 0.0,
            },
        }
    }

    /// The advance width of one character.
    pub(crate) fn advance(&self, ch: char, style: FontStyle) -> f32 {
        self.face(style.weight).metrics(ch, style.size).advance_width
    }

    /// Advances the pen by `text`'s glyphs.
    ///
    /// The embedded faces carry no legacy `kern` table, so pair adjustment is
    /// not available; per-glyph advances are the browser result for the
    /// scripts the subset covers.
    pub(crate) fn measure(&self, text: &str, style: FontStyle) -> f32 {
        text.chars().map(|ch| self.advance(ch, style)).sum()
    }

    /// The coverage bitmap and metrics for one glyph.
    pub(crate) fn rasterize(&self, ch: char, style: FontStyle) -> (Metrics, Vec<u8>) {
        self.face(style.weight).rasterize(ch, style.size)
    }
}
