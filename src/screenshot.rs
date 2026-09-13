//! Spike: one-shot HTML file -> PNG through the Blitz stack.
//!
//! Feature-gated (`screenshot`) and never enabled in shipping builds. The
//! standalone probe under `spikes/blitz-screenshot` carries the same pipeline;
//! this module exists so the size cost is measured on the real binary.
//!
//! Flow: html5ever parse -> Stylo cascade -> Taffy layout -> `anyrender`/`vello_cpu`
//! raster -> PNG. One document, one buffer, no window, no animation. See
//! `projects/tinybrowser/screenshot.md` (Obsidian) for the product note.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, build_single_font_ctx};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use peniko::kurbo::{Affine, Rect};
use peniko::{Color, Fill};

const VIEWPORT_WIDTH: u32 = 1000;
const VIEWPORT_HEIGHT: u32 = 800;
/// Crop tall pages at this height, in CSS pixels.
const MAX_RENDER_HEIGHT: f32 = 4000.0;

/// Render `html_path` to `out_path`, resolving text with `font_path`.
pub fn run(html_path: &Path, out_path: &Path, font_path: &Path) -> ExitCode {
    match render(html_path, out_path, font_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            logging::error!(target: "screenshot", "{error}");
            ExitCode::from(1)
        }
    }
}

fn render(html_path: &Path, out_path: &Path, font_path: &Path) -> Result<(), String> {
    let html = std::fs::read_to_string(html_path)
        .map_err(|error| format!("{}: {error}", html_path.display()))?;
    let font =
        std::fs::read(font_path).map_err(|error| format!("{}: {error}", font_path.display()))?;

    let mut document = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            viewport: Some(Viewport::new(
                VIEWPORT_WIDTH,
                VIEWPORT_HEIGHT,
                1.0,
                ColorScheme::Light,
            )),
            // One font, system discovery off: no fontconfig, no font dir
            // scanning. A shipping build would embed a subset instead.
            font_ctx: Some(build_single_font_ctx(&font)),
            ..Default::default()
        },
    );

    document.resolve(0.0);
    document.resolve_layout();

    let content_height = document.root_element().final_layout().size.height;
    let render_height = clamp_pixels(content_height).max(VIEWPORT_HEIGHT);

    let buffer = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            scene.fill(
                Fill::NonZero,
                Affine::default(),
                Color::WHITE,
                Option::default(),
                &Rect::new(
                    0.0,
                    0.0,
                    f64::from(VIEWPORT_WIDTH),
                    f64::from(render_height),
                ),
            );
            paint_scene(
                scene,
                document.as_mut(),
                1.0,
                VIEWPORT_WIDTH,
                render_height,
                0,
                0,
            );
        },
        VIEWPORT_WIDTH,
        render_height,
    );

    write_png(out_path, &buffer, render_height)
}

fn write_png(out_path: &Path, buffer: &[u8], height: u32) -> Result<(), String> {
    let file =
        File::create(out_path).map_err(|error| format!("{}: {error}", out_path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), VIEWPORT_WIDTH, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
    writer
        .write_image_data(buffer)
        .map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())
}

/// Resolved content height in CSS pixels, clamped to the render ceiling.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to [0, MAX_RENDER_HEIGHT] first, so the cast is exact and non-negative"
)]
fn clamp_pixels(content_height: f32) -> u32 {
    content_height.clamp(0.0, MAX_RENDER_HEIGHT) as u32
}
