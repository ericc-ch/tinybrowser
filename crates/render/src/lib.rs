//! One-shot HTML/CSS rendering for screenshots.
//!
//! The pipeline mirrors how minimal browsers layer their engines (`NetSurf`'s
//! `libcss` -> `hubbub` -> layout -> `nsfb` paint, Dillo's style -> layout ->
//! canvas, Obscura's `obscura-render` over Taffy): style the DOM, build a box
//! tree, lay it out, paint a display list into an RGBA buffer, encode once.
//! Nothing here is incremental; a screenshot runs the whole pipeline per call.
//!
//! Scope on the cascade side is the whole CSS cascade: Stylo owns selector
//! matching, inheritance, and computed values. What stays a deliberate subset
//! is layout and paint — the property mappings in `stylo_map.rs` feed only
//! what the box tree, Taffy, Parley, and the CPU painter implement, and the
//! unsupported cases are documented instead of approximated silently.
//!
//! Layout is split the way the CSS formatting model is:
//!
//! - block containers stack in-flow children and establish the containing
//!   block (<https://drafts.csswg.org/css2/#visuren>)
//! - inline formatting lays out line boxes of text and inline boxes
//!   (<https://drafts.csswg.org/css2/#inline-formatting>)
//! - flex containers use the flex layout algorithm
//!   (<https://drafts.csswg.org/css-flexbox-1/#layout-algorithm>)
//!
//! The DOM arrives as an immutable [`dom::Dom`]; the output is a
//! [`RgbaImage`] ready for PNG encoding in [`png`].

#![doc = include_str!("../README.md")]

mod cascade;
mod color;
mod stylo;
mod stylo_map;
mod stylo_view;

mod boxes;
mod font;
mod geometry;
mod layout;
mod paint;
mod png;
mod style;
mod text;
mod tree;

pub use color::Color;
pub use png::encode_png;

/// One rendered viewport, 8 bits per channel, straight (non-premultiplied)
/// alpha, row-major RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA order.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Crops to the device-pixel rectangle `(x, y, width, height)`, clamped
    /// to the image bounds.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidCrop`] when the rectangle has no area after
    /// clamping.
    pub fn crop(&self, x: f32, y: f32, width: f32, height: f32) -> Result<Self, RenderError> {
        let image_width = pixels(self.width);
        let image_height = pixels(self.height);
        let left = x.max(0.0).min(image_width).floor();
        let top = y.max(0.0).min(image_height).floor();
        let right = (x + width).max(left).min(image_width).ceil();
        let bottom = (y + height).max(top).min(image_height).ceil();
        let crop_width = device_pixels(right - left);
        let crop_height = device_pixels(bottom - top);
        if crop_width == 0 || crop_height == 0 {
            return Err(RenderError::InvalidCrop);
        }
        let left = device_pixels(left);
        let top = device_pixels(top);
        let mut data = Vec::with_capacity((crop_width * crop_height * 4) as usize);
        for row in 0..crop_height {
            let start = (((top + row) * self.width + left) * 4) as usize;
            let end = start + (crop_width * 4) as usize;
            data.extend_from_slice(&self.data[start..end]);
        }
        Ok(Self {
            width: crop_width,
            height: crop_height,
            data,
        })
    }
}

/// Renders `dom` through the style/layout/paint pipeline into one image.
///
/// Stylesheets found in the document (`<style>`) and any externally fetched
/// sheets passed by the caller are applied in document order. The viewport is
/// the visual viewport in CSS pixels; `scale` is the device pixel ratio
/// (<https://drafts.csswg.org/cssom-view/#dom-window-devicepixelratio>).
///
/// # Errors
///
/// Returns [`RenderError`] when a stylesheet cannot be parsed or the pipeline
/// refuses a document it cannot lay out.
pub fn render(
    dom: &dom::Dom,
    stylesheets: &[String],
    options: &RenderOptions,
) -> Result<RgbaImage, RenderError> {
    cascade::render(dom, stylesheets, options)
}

/// Viewport and device parameters for one render.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Viewport width in CSS pixels.
    pub width: f32,
    /// Viewport height in CSS pixels.
    pub height: f32,
    /// Device pixel ratio; output size is the viewport times this factor.
    pub scale: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 800.0,
            height: 600.0,
            scale: 1.0,
        }
    }
}

/// A render failure that is the document's fault rather than the caller's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The viewport or scale was not a positive, finite size.
    InvalidViewport,
    /// The output image would exceed the renderer's size cap.
    TooLarge,
    /// An embedded font failed to parse; the build is corrupt.
    Font,
    /// The PNG encoder rejected the rendered image.
    Encode,
    /// A crop rectangle had no area inside the image.
    InvalidCrop,
}

/// Rounds a CSS pixel dimension to device pixels.
///
/// Callers pass finite, non-negative values; the pixel cap bounds the result
/// well below `u32`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers clamp to finite non-negative values under MAX_PIXELS"
)]
pub(crate) fn device_pixels(value: f32) -> u32 {
    value as u32
}

/// Converts a device-pixel coordinate or dimension back to `f32`.
#[expect(
    clippy::cast_precision_loss,
    reason = "device dimensions are capped at 4096, exact in f32"
)]
pub(crate) fn pixels(value: u32) -> f32 {
    value as f32
}

/// A DOM or list count as a layout number.
#[expect(
    clippy::cast_precision_loss,
    reason = "element, line, and item counts stay far below 2^24; exact in f32"
)]
pub(crate) fn count(value: usize) -> f32 {
    value as f32
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidViewport => f.write_str("invalid viewport"),
            Self::TooLarge => f.write_str("render output exceeds the size cap"),
            Self::Font => f.write_str("embedded font failed to parse"),
            Self::Encode => f.write_str("png encoding failed"),
            Self::InvalidCrop => f.write_str("crop rectangle is empty"),
        }
    }
}

impl std::error::Error for RenderError {}
