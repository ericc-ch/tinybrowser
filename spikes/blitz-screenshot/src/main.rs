//! One-shot screenshot probe: HTML file -> Blitz parse/style/layout -> paint
//! via anyrender + vello_cpu -> PNG.
//!
//! Usage: blitz-screenshot <in.html> <out.png> <font.ttf> [width] [height]

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

use anyrender::{PaintScene as _, render_to_buffer};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, build_single_font_ctx};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use peniko::kurbo::Rect;
use peniko::{Color, Fill};

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let html_path = args.next().ok_or("usage: blitz-screenshot <in.html> <out.png> <font.ttf> [w] [h]")?;
    let out_path = args.next().ok_or("missing <out.png>")?;
    let font_path = args.next().ok_or("missing <font.ttf>")?;
    let width: u32 = args.next().map_or(Ok(1000), |s| s.parse()).map_err(|e| format!("bad width: {e}"))?;
    let height: u32 = args.next().map_or(Ok(800), |s| s.parse()).map_err(|e| format!("bad height: {e}"))?;
    let scale = 1.0f32;

    let html = std::fs::read_to_string(&html_path).map_err(|e| format!("read {html_path}: {e}"))?;
    let font = std::fs::read(&font_path).map_err(|e| format!("read {font_path}: {e}"))?;

    // One embedded/loaded font, system discovery off: no fontconfig either way.
    let font_ctx = build_single_font_ctx(&font);

    let mut document = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            viewport: Some(Viewport::new(width, height, scale, ColorScheme::Light)),
            font_ctx: Some(font_ctx),
            ..Default::default()
        },
    );

    // Style and layout the whole document once.
    document.resolve(0.0);
    document.resolve_layout();

    let content_height = document.root_element().final_layout().size.height;
    let render_height = content_height.max(height as f32).min(4000.0) as u32;

    let buffer = render_to_buffer::<VelloCpuImageRenderer, _>(
        |scene| {
            scene.fill(
                Fill::NonZero,
                Default::default(),
                Color::WHITE,
                Default::default(),
                &Rect::new(0.0, 0.0, f64::from(width), f64::from(render_height)),
            );
            paint_scene(
                scene,
                document.as_mut(),
                f64::from(scale),
                width,
                render_height,
                0,
                0,
            );
        },
        width,
        render_height,
    );

    let file = File::create(&out_path).map_err(|e| format!("create {out_path}: {e}"))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, render_height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png header: {e}"))?;
    writer.write_image_data(&buffer).map_err(|e| format!("png data: {e}"))?;
    writer.finish().map_err(|e| format!("png finish: {e}"))?;

    println!("wrote {out_path} ({width}x{render_height})");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
