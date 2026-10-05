//! Decode one `<img>` body into premultiplied RGBA.
//!
//! HTML does not list required image types
//! (<https://html.spec.whatwg.org/multipage/images.html#alt>). The user
//! agent sniffs, then decodes the types it supports
//! (<https://html.spec.whatwg.org/multipage/images.html#content-type-sniffing>,
//! <https://mimesniff.spec.whatwg.org/#matching-an-image-type-pattern>).
//! This crate supports JPEG, PNG, and WebP.
//!
//! Known gap: SVG images decode for paint inside Blitz but have no decoded
//! pixels in our own image map, so an SVG `<img>` fires `error` instead of
//! `load` until SVG rasterization is rewired onto the Blitz stack.

use std::io::Cursor;

use image_webp::WebPDecoder;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::render::png::{decode_png, push_premultiplied};
use crate::render::{MAX_DECODED_SIDE, RasterImage, decoded_rgba_fits};

/// Sniffed raster type from the MIME sniff image-pattern table.
enum RasterKind {
    Jpeg,
    Png,
    Webp,
}

/// Decodes JPEG, PNG, or WebP bytes into one bitmap.
pub(crate) fn decode_image(bytes: &[u8]) -> Option<RasterImage> {
    match sniff_raster(bytes) {
        Some(RasterKind::Png) => decode_png(bytes),
        Some(RasterKind::Jpeg) => decode_jpeg(bytes),
        Some(RasterKind::Webp) => decode_webp(bytes),
        // SVG and unknown bytes have no decoder here; the caller reports the
        // request as broken (see the module docs).
        None => None,
    }
}

/// Matches JPEG, PNG, and WebP magic bytes
/// (<https://mimesniff.spec.whatwg.org/#matching-an-image-type-pattern>).
fn sniff_raster(bytes: &[u8]) -> Option<RasterKind> {
    if matches_mask(
        bytes,
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
    ) {
        return Some(RasterKind::Png);
    }
    if matches_mask(bytes, &[0xFF, 0xD8, 0xFF], &[0xFF, 0xFF, 0xFF]) {
        return Some(RasterKind::Jpeg);
    }
    if matches_mask(
        bytes,
        &[
            0x52, 0x49, 0x46, 0x46, 0x00, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50,
        ],
        &[
            0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ],
    ) {
        return Some(RasterKind::Webp);
    }
    None
}

fn matches_mask(bytes: &[u8], pattern: &[u8], mask: &[u8]) -> bool {
    if bytes.len() < pattern.len() {
        return false;
    }
    bytes
        .iter()
        .zip(pattern)
        .zip(mask)
        .all(|((byte, expected), bits)| byte & bits == expected & bits)
}

fn decode_jpeg(bytes: &[u8]) -> Option<RasterImage> {
    let options = DecoderOptions::default()
        .set_use_unsafe(false)
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(usize::try_from(MAX_DECODED_SIDE).ok()?)
        .set_max_height(usize::try_from(MAX_DECODED_SIDE).ok()?);
    let mut decoder = JpegDecoder::new_with_options(Cursor::new(bytes), options);
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    if !decoded_rgba_fits(width, height) {
        return None;
    }
    let pixels = decoder.decode().ok()?;
    rgba_bytes(&pixels, width, height)
}

fn decode_webp(bytes: &[u8]) -> Option<RasterImage> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
    decoder.set_memory_limit(crate::render::MAX_DECODED_IMAGE_BYTES);
    let (width, height) = decoder.dimensions();
    if !decoded_rgba_fits(width, height) {
        return None;
    }
    let size = decoder.output_buffer_size()?;
    let mut source = vec![0_u8; size];
    decoder.read_image(&mut source).ok()?;
    if decoder.has_alpha() {
        rgba_bytes(&source, width, height)
    } else {
        rgb_bytes(&source, width, height)
    }
}

fn rgb_bytes(source: &[u8], width: u32, height: u32) -> Option<RasterImage> {
    let pixels = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    let mut data = Vec::with_capacity(pixels.checked_mul(4)?);
    for &[r, g, b] in source.as_chunks::<3>().0 {
        data.extend_from_slice(&[r, g, b, 255]);
    }
    (data.len() == pixels * 4).then_some(RasterImage {
        width,
        height,
        data,
    })
}

fn rgba_bytes(source: &[u8], width: u32, height: u32) -> Option<RasterImage> {
    let pixels = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    let mut data = Vec::with_capacity(pixels.checked_mul(4)?);
    for &[r, g, b, a] in source.as_chunks::<4>().0 {
        push_premultiplied(&mut data, r, g, b, a);
    }
    (data.len() == pixels * 4).then_some(RasterImage {
        width,
        height,
        data,
    })
}
