//! PNG decoding for page images and encoding for screenshots.

use std::io::Cursor;

use png::{BitDepth, ColorType, Decoder, Encoder, Limits, Transformations};

use crate::render::{RasterImage, RenderError, RgbaImage};

/// Maximum temporary decoder allocation for one page image.
const MAX_DECODE_BYTES: usize = 32 * 1024 * 1024;

/// Decodes the first PNG frame to premultiplied 8-bit RGBA.
pub(crate) fn decode_png(bytes: &[u8]) -> Option<RasterImage> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(
        Transformations::EXPAND | Transformations::ALPHA | Transformations::normalize_to_color8(),
    );
    decoder.set_limits(Limits {
        bytes: MAX_DECODE_BYTES,
    });
    let mut reader = decoder.read_info().ok()?;
    let mut source = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut source).ok()?;
    let source = &source[..info.buffer_size()];
    let pixels = usize::try_from(info.width)
        .ok()?
        .checked_mul(usize::try_from(info.height).ok()?)?;
    let mut data = Vec::with_capacity(pixels.checked_mul(4)?);
    match info.color_type {
        ColorType::Rgba => {
            for &[r, g, b, a] in source.as_chunks::<4>().0 {
                push_premultiplied(&mut data, r, g, b, a);
            }
        }
        ColorType::Rgb => {
            for &[r, g, b] in source.as_chunks::<3>().0 {
                data.extend_from_slice(&[r, g, b, 255]);
            }
        }
        ColorType::GrayscaleAlpha => {
            for &[value, alpha] in source.as_chunks::<2>().0 {
                push_premultiplied(&mut data, value, value, value, alpha);
            }
        }
        ColorType::Grayscale => {
            for &value in source {
                data.extend_from_slice(&[value, value, value, 255]);
            }
        }
        ColorType::Indexed => return None, // EXPAND should have rewritten this.
    }
    (data.len() == pixels * 4).then_some(RasterImage {
        width: info.width,
        height: info.height,
        data,
    })
}

pub(super) fn push_premultiplied(out: &mut Vec<u8>, r: u8, g: u8, b: u8, a: u8) {
    let premultiply = |channel: u8| {
        let product = u16::from(channel) * u16::from(a) + 127;
        u8::try_from(product / 255).unwrap_or(255)
    };
    out.extend_from_slice(&[premultiply(r), premultiply(g), premultiply(b), a]);
}

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
