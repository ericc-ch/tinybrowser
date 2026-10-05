//! Screenshots through Blitz: resolve, paint, encode.

mod blitz;
mod decode;
mod png;
mod providers;

pub(crate) use blitz::{INVALID_BASE_URL, blitz_base_url, paint, resolve_until_settled};
pub(crate) use decode::decode_image;
pub(crate) use providers::{BlitzFetch, CountingHandler, TinyNav, TinyNetProvider, TinyShell};
pub use png::encode_png;

/// Aggregate retained decoded image pixels, and the decode cap for one bitmap.
pub(crate) const MAX_DECODED_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum width or height of a decoded page image.
pub(crate) const MAX_DECODED_SIDE: u32 = 4096;

/// Whether a premultiplied RGBA bitmap of `width`×`height` fits the decode
/// and store budget. 4096×4096 RGBA is ~67MiB, over `MAX_DECODED_IMAGE_BYTES`.
pub(crate) fn decoded_rgba_fits(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 || width > MAX_DECODED_SIDE || height > MAX_DECODED_SIDE {
        return false;
    }
    let Ok(width) = usize::try_from(width) else {
        return false;
    };
    let Ok(height) = usize::try_from(height) else {
        return false;
    };
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .is_some_and(|bytes| bytes <= MAX_DECODED_IMAGE_BYTES)
}

/// One decoded image in premultiplied RGBA form.
#[derive(Clone, Debug)]
pub(crate) struct RasterImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data: Vec<u8>,
}

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

/// One laid-out box in CSS pixels, keyed by its DOM element when it has one.
#[derive(Clone, Copy, Debug)]
pub struct NodeBox {
    /// The element the box was generated for.
    pub node: Option<crate::js::world::NodeId>,
    /// Border box in CSS pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Whether the originating style is visible. Hidden boxes still report
    /// geometry, but hit testing skips them (descendants override).
    pub visible: bool,
}

/// A render failure that is the document's fault rather than the caller's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The viewport or scale was not a positive, finite size.
    InvalidViewport,
    /// The output image would exceed the renderer's size cap.
    TooLarge,
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

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidViewport => f.write_str("invalid viewport"),
            Self::TooLarge => f.write_str("render output exceeds the size cap"),
            Self::Encode => f.write_str("png encoding failed"),
            Self::InvalidCrop => f.write_str("crop rectangle is empty"),
        }
    }
}

impl std::error::Error for RenderError {}
