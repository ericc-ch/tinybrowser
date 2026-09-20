//! Straight-alpha 8-bit sRGB colors.
//!
//! CSS color parsing and resolution live in Stylo; this type is the paint
//! pipeline's output format (`tiny-skia` fills blend it directly).
//!
//! `Color` is the computed, absolute result: `currentColor`, `color-mix()`,
//! relative color syntax, and the global keywords are resolved during the
//! cascade before a value reaches paint.

/// A straight-alpha 8-bit sRGB color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel; 255 is opaque.
    pub a: u8,
}

impl Color {
    /// Opaque black, the initial value of `color`.
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// An opaque color.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A color with explicit alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}
