//! PNG encoding for rendered images.

use png::{BitDepth, ColorType, Encoder};

use crate::render::{RenderError, RgbaImage};

/// Encodes one RGBA image as a PNG.
///
/// # Errors
///
/// [`RenderError::Encode`] when the encoder rejects the image; the pipeline
/// only produces 8-bit RGBA, so a failure means the image dimensions
/// overflow the format limits.
pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, RenderError> {
    let mut out = Vec::new();
    {
        let mut encoder = Encoder::new(&mut out, image.width, image.height);
        encoder.set_color(ColorType::Rgba);
        encoder.set_depth(BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|_| RenderError::Encode)?;
        writer
            .write_image_data(&image.data)
            .map_err(|_| RenderError::Encode)?;
    }
    Ok(out)
}
