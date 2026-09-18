//! Embedded text faces.
//!
//! The shipping binary carries subset fonts instead of reading system fonts:
//! one binary, no fontconfig, deterministic output. Liberation Sans is
//! metric-compatible with Arial and OFL-1.1 licensed; the subset covers
//! Latin plus the punctuation and symbols web pages actually use, and its
//! license text ships next to the faces (`assets/OFL.txt`).
//!
//! Two consumers share the faces: `fontdue` answers intrinsic width queries,
//! and Parley shapes and breaks real lines over a `fontique` collection with
//! the same bytes registered from memory. Glyph pixels come from `skrifa`
//! outlines filled by `tiny-skia`, so shaped glyph IDs paint exactly as the
//! shaper positioned them.

use std::cell::RefCell;
use std::sync::Arc;

use fontdue::{Font, FontSettings};

use crate::RenderError;

/// Regular-weight face bytes, embedded at compile time.
const REGULAR: &[u8] = include_bytes!("../assets/LiberationSans-Regular.ttf");

/// Bold-weight face bytes, embedded at compile time.
const BOLD: &[u8] = include_bytes!("../assets/LiberationSans-Bold.ttf");

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
    regular: Font,
    bold: Font,
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
            regular: Font::from_bytes(REGULAR, FontSettings::default())
                .map_err(|_| RenderError::Font)?,
            bold: Font::from_bytes(BOLD, FontSettings::default()).map_err(|_| RenderError::Font)?,
            parley: RefCell::new(ParleyFonts::load()),
            skrifa_regular: skrifa::FontRef::new(REGULAR).map_err(|_| RenderError::Font)?,
            skrifa_bold: skrifa::FontRef::new(BOLD).map_err(|_| RenderError::Font)?,
        })
    }

    /// The fontdue face for `weight`, used for intrinsic measurements.
    pub(crate) fn face(&self, weight: Weight) -> &Font {
        match weight {
            Weight::Normal => &self.regular,
            Weight::Bold => &self.bold,
        }
    }

    /// The skrifa face for `weight`, used to draw shaped glyph outlines.
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

    #[test]
    fn embedded_faces_parse_and_rasterize() {
        let fonts = Fonts::load().expect("embedded fonts");
        for weight in [Weight::Normal, Weight::Bold] {
            let face = fonts.face(weight);
            let (metrics, bitmap) = face.rasterize('W', 16.0);
            assert!(metrics.width > 0);
            assert!(metrics.advance_width > 0.0);
            assert_eq!(bitmap.len(), metrics.width * metrics.height);
        }
    }
}
