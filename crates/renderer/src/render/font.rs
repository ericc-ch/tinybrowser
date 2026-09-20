//! Embedded text faces.
//!
//! The shipping binary carries subset fonts instead of reading system fonts:
//! one binary, no fontconfig, deterministic output. Liberation Sans is
//! metric-compatible with Arial and OFL-1.1 licensed; the subset covers
//! Latin plus the punctuation and symbols web pages actually use, and its
//! license text ships next to the faces (`crates/renderer/assets/OFL.txt`).
//!
//! Parley shapes and breaks real lines over a `fontique` collection with the
//! face bytes registered from memory. Glyph pixels and intrinsic advances
//! both come from `skrifa` — outlines filled by `tiny-skia` for shaped glyph
//! IDs, advance widths for measurement — so one font stack covers the whole
//! pipeline.

use std::cell::RefCell;
use std::sync::Arc;

use crate::render::RenderError;

/// Regular-weight face bytes, embedded at compile time.
const REGULAR: &[u8] = include_bytes!("../../assets/LiberationSans-Regular.ttf");

/// Bold-weight face bytes, embedded at compile time.
const BOLD: &[u8] = include_bytes!("../../assets/LiberationSans-Bold.ttf");

/// Regular-weight face bytes for consumers outside [`Fonts`], like Stylo's
/// font-metrics provider.
pub(crate) const REGULAR_BYTES: &[u8] = REGULAR;

/// Bold-weight face bytes for consumers outside [`Fonts`].
pub(crate) const BOLD_BYTES: &[u8] = BOLD;

/// A face's vertical metrics and key advances at one size, for Stylo's
/// font-relative units.
pub(crate) struct FaceMetrics {
    /// Ascent in pixels.
    pub ascent: f32,
    /// X-height in pixels, if the face reports one.
    pub x_height: Option<f32>,
    /// Cap-height in pixels, if the face reports one.
    pub cap_height: Option<f32>,
    /// Advance of `0` in pixels, for `ch`.
    pub zero_advance: Option<f32>,
    /// Advance of U+6C34 in pixels, for `ic`.
    pub ic_width: Option<f32>,
}

/// Reads one face's metrics at `px` pixels for `location` in variation space.
///
/// The embedded faces always parse; `None` fields mark what the face does
/// not report, and Stylo falls back without them.
pub(crate) fn metrics_for(
    bytes: &'static [u8],
    px: f32,
    location: skrifa::instance::LocationRef<'_>,
) -> FaceMetrics {
    use skrifa::MetadataProvider as _;
    let size = skrifa::instance::Size::new(px);
    let mut out = FaceMetrics {
        ascent: px,
        x_height: None,
        cap_height: None,
        zero_advance: None,
        ic_width: None,
    };
    let Ok(face) = skrifa::FontRef::new(bytes) else {
        return out;
    };
    let metrics = skrifa::metrics::Metrics::new(&face, size, location);
    out.ascent = metrics.ascent;
    out.x_height = metrics.x_height;
    out.cap_height = metrics.cap_height;
    let glyphs = skrifa::metrics::GlyphMetrics::new(&face, size, location);
    let charmap = face.charmap();
    if let Some(id) = charmap.map('0') {
        out.zero_advance = glyphs.advance_width(id);
    }
    if let Some(id) = charmap.map('\u{6C34}') {
        out.ic_width = glyphs.advance_width(id);
    }
    out
}

/// Font weight axis, limited to the two weights the binary carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weight {
    /// `font-weight: normal` (400).
    Normal,
    /// `font-weight: bold` (700) and above.
    Bold,
}

/// Parley's font and layout contexts over the embedded faces.
///
/// One per render: shaping borrows it mutably, and the borrow discipline in
/// the measure path keeps borrows strictly sequential (atomic measurement
/// finishes before line shaping starts), so a shared `RefCell` never nests.
pub(crate) struct ParleyFonts {
    /// Font collection with both faces registered from memory.
    pub(crate) font_context: parley::FontContext,
    /// Layout context with warm shaping caches across one render.
    pub(crate) layout_context: parley::LayoutContext<[u8; 4]>,
}

impl ParleyFonts {
    /// Registers the embedded faces with no system font discovery.
    fn load() -> Self {
        let mut font_context = parley::FontContext {
            collection: fontique::Collection::default(),
            source_cache: fontique::SourceCache::default(),
        };
        for face in [REGULAR, BOLD] {
            font_context
                .collection
                .register_fonts(fontique::Blob::new(Arc::new(face.to_vec())), None);
        }
        Self {
            font_context,
            layout_context: parley::LayoutContext::new(),
        }
    }
}

/// The parsed faces of one render.
pub(crate) struct Fonts {
    /// Parley shaping state, shared by every text leaf of one render.
    pub(crate) parley: RefCell<ParleyFonts>,
    skrifa_regular: skrifa::FontRef<'static>,
    skrifa_bold: skrifa::FontRef<'static>,
}

impl Fonts {
    /// Parses the embedded faces.
    ///
    /// Called per render: the faces are small and parsing is cheap, so no
    /// global cache has to be invalidated or locked.
    ///
    /// # Errors
    ///
    /// [`RenderError::Font`] when an embedded face fails to parse, which
    /// means the build is corrupt rather than the page.
    pub(crate) fn load() -> Result<Self, RenderError> {
        Ok(Self {
            parley: RefCell::new(ParleyFonts::load()),
            skrifa_regular: skrifa::FontRef::new(REGULAR).map_err(|_| RenderError::Font)?,
            skrifa_bold: skrifa::FontRef::new(BOLD).map_err(|_| RenderError::Font)?,
        })
    }

    /// The skrifa face for `weight`: outlines for paint, advances for
    /// intrinsic width measurement.
    pub(crate) fn outline_face(&self, weight: Weight) -> skrifa::FontRef<'static> {
        match weight {
            Weight::Normal => self.skrifa_regular.clone(),
            Weight::Bold => self.skrifa_bold.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Fonts, Weight};
    use skrifa::MetadataProvider as _;

    #[test]
    fn embedded_faces_parse_and_map_glyphs() {
        let fonts = Fonts::load().expect("embedded fonts");
        for weight in [Weight::Normal, Weight::Bold] {
            let face = fonts.outline_face(weight);
            let glyph = face.charmap().map('W').expect("embedded faces cover W");
            assert!(glyph.to_u32() > 0);
        }
    }
}
