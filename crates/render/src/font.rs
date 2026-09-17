//! Embedded text faces.
//!
//! The shipping binary carries subset fonts instead of reading system fonts:
//! one binary, no fontconfig, deterministic output. Liberation Sans is
//! metric-compatible with Arial and OFL-1.1 licensed; the subset covers
//! Latin plus the punctuation and symbols web pages actually use, and its
//! license text ships next to the faces (`assets/OFL.txt`).

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

/// The parsed faces of one render.
pub(crate) struct Fonts {
    regular: Font,
    bold: Font,
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
        })
    }

    /// The face for `weight`.
    pub(crate) fn face(&self, weight: Weight) -> &Font {
        match weight {
            Weight::Normal => &self.regular,
            Weight::Bold => &self.bold,
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
