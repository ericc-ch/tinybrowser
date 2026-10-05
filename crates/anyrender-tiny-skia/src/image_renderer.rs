//! [`anyrender::ImageRenderer`] over the immediate [`tiny_skia`] painter.

use anyrender::{ImageRenderer, PaintScene as _, RenderContext};

use crate::TinySkiaScenePainter;

/// Renderer drawing one scene into an RGBA8 buffer.
pub struct TinySkiaImageRenderer {
    painter: TinySkiaScenePainter,
}

impl RenderContext for TinySkiaImageRenderer {}

impl ImageRenderer for TinySkiaImageRenderer {
    type ScenePainter<'a> = TinySkiaScenePainter;

    fn new(width: u32, height: u32) -> Self {
        Self {
            painter: TinySkiaScenePainter::new(width, height),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.painter = TinySkiaScenePainter::new(width, height);
    }

    fn reset(&mut self) {
        self.painter.reset();
    }

    fn render<F: FnOnce(&mut Self::ScenePainter<'_>)>(&mut self, draw_fn: F, buffer: &mut [u8]) {
        draw_fn(&mut self.painter);
        let data = self.painter.pixmap().data();
        let len = buffer.len().min(data.len());
        buffer[..len].copy_from_slice(&data[..len]);
    }

    fn render_to_vec<F: FnOnce(&mut Self::ScenePainter<'_>)>(
        &mut self,
        draw_fn: F,
        buffer: &mut Vec<u8>,
    ) {
        draw_fn(&mut self.painter);
        let data = self.painter.pixmap().data();
        buffer.resize(data.len(), 0);
        buffer.copy_from_slice(data);
    }
}
