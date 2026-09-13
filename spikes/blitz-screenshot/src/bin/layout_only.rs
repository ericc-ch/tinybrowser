//! Size-ladder rung: parse + style + layout only. No paint backend, no PNG.
//!
//! Usage: layout_only <in.html> <font.ttf>

use std::env;
use std::process::ExitCode;

use blitz_dom::{DocumentConfig, build_single_font_ctx};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(html_path) = args.next() else {
        eprintln!("usage: layout_only <in.html> <font.ttf>");
        return ExitCode::FAILURE;
    };
    let Some(font_path) = args.next() else {
        eprintln!("usage: layout_only <in.html> <font.ttf>");
        return ExitCode::FAILURE;
    };

    let html = std::fs::read_to_string(&html_path).expect("read html");
    let font = std::fs::read(&font_path).expect("read font");

    let mut document = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            viewport: Some(Viewport::new(1000, 800, 1.0, ColorScheme::Light)),
            font_ctx: Some(build_single_font_ctx(&font)),
            ..Default::default()
        },
    );
    document.resolve(0.0);
    document.resolve_layout();

    let height = document.root_element().final_layout().size.height;
    println!("laid out; content height {height}px");
    ExitCode::SUCCESS
}
