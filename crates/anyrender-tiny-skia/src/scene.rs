//! Immediate-mode [`tiny_skia`] scene painter.
//!
//! One painter owns the canvas pixmap plus a stack of pushed layers. Every
//! draw resolves its brush, applies the current transform and the current
//! clip mask, and paints straight into the top target. Layers paint into a
//! transparent child pixmap that composites back onto its parent on pop,
//! which is also where the layer filter runs.

use std::sync::Arc;

use anyrender::{Filter, Glyph, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use kurbo::{Affine, PathEl, Shape, Stroke};
use peniko::{BlendMode, Color, Fill, FontData, StyleRef};
use tiny_skia::{
    BlendMode as SkiaBlend, FillRule as SkiaRule, IntSize, Mask, Paint as SkiaPaint, Path,
    PathBuilder, Pixmap, PixmapPaint, SpreadMode as SkiaSpread, Stroke as SkiaStroke,
    StrokeDash as SkiaDash, Transform as SkiaXform,
};

/// Tolerance for flattening curves into a [`tiny_skia`] path, matching the
/// `vello_cpu` backend's flattening.
const TOLERANCE: f64 = 0.1;

/// Largest pixmap side this painter allocates. The renderer caps captures
/// well below this; anything larger clamps instead of failing.
const MAX_SIDE: u32 = 8192;

/// Builds a pixmap, clamping hostile dimensions. Clamped sides always
/// allocate; the 1x1 fallback only runs when the allocator itself fails.
fn make_pixmap(width: u32, height: u32) -> Pixmap {
    let width = width.clamp(1, MAX_SIDE);
    let height = height.clamp(1, MAX_SIDE);
    if let Some(pixmap) = Pixmap::new(width, height) {
        return pixmap;
    }
    Pixmap::new(1, 1).unwrap_or_else(|| {
        // A four-byte allocation cannot realistically fail; reaching here
        // means the process is out of memory.
        unreachable!("1x1 pixmap allocation failed");
    })
}

/// One drawing surface: its pixels, the paint compositing it onto its
/// parent (the canvas uses an opaque source-over), and its clip stack.
struct Target {
    pixmap: Pixmap,
    paint: PixmapPaint,
    clips: Vec<Mask>,
}

/// Painter drawing an [`anyrender`] scene into a [`tiny_skia`] pixmap.
pub struct TinySkiaScenePainter {
    transform: SkiaXform,
    targets: Vec<Target>,
    /// Converted image for the in-progress image fill. A [`tiny_skia`]
    /// `Pattern` borrows its pixels, so the pixmap lives here instead of a
    /// local; draw code reaches it through disjoint field borrows.
    image_scratch: Option<Pixmap>,
}

impl TinySkiaScenePainter {
    /// Builds a `width` x `height` painter cleared to transparent black.
    /// The caller composites onto its own background (screenshots flatten
    /// over white in the renderer).
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let pixmap = make_pixmap(width, height);
        let paint = PixmapPaint::default();
        Self {
            transform: SkiaXform::identity(),
            targets: vec![Target {
                pixmap,
                paint,
                clips: Vec::new(),
            }],
            image_scratch: None,
        }
    }

    /// Canvas pixels, premultiplied RGBA.
    #[must_use]
    pub fn pixmap(&self) -> &Pixmap {
        &self.targets[0].pixmap
    }

    /// Takes the canvas pixels, leaving the painter otherwise intact.
    pub fn take_pixmap(&mut self) -> Pixmap {
        let (width, height) = {
            let canvas = &self.targets[0].pixmap;
            (canvas.width(), canvas.height())
        };
        let fresh = make_pixmap(width, height);
        std::mem::replace(&mut self.targets[0].pixmap, fresh)
    }

    /// Current drawing surface and its active clip mask, via disjoint field
    /// borrows so both can be used in one draw call. The canvas target is
    /// never popped, so the stack is never empty.
    fn draw_target(&mut self) -> (&mut Pixmap, Option<&Mask>) {
        let Some(target) = self.targets.last_mut() else {
            unreachable!("the canvas target is never popped");
        };
        let clip = target.clips.last();
        (&mut target.pixmap, clip)
    }

    /// Fills `path` with an image brush, converting on demand. Returns false
    /// when the bytes are unusable, so the caller falls back to transparent.
    /// `tiny-skia` patterns take one spread mode; mismatched axes read the
    /// horizontal one.
    fn fill_image(
        &mut self,
        image: &peniko::ImageBrushRef<'_>,
        brush_transform: Option<Affine>,
        style: Fill,
        path: &Path,
    ) -> bool {
        let data = &image.image;
        let Some(converted) = image_to_pixmap(
            data.data.data(),
            data.format,
            data.alpha_type,
            data.width,
            data.height,
        ) else {
            return false;
        };
        self.image_scratch = Some(converted);
        let Some(scratch) = self.image_scratch.as_ref() else {
            return false;
        };
        let spread = match image.sampler.x_extend {
            peniko::Extend::Pad => SkiaSpread::Pad,
            peniko::Extend::Repeat => SkiaSpread::Repeat,
            peniko::Extend::Reflect => SkiaSpread::Reflect,
        };
        let quality = match image.sampler.quality {
            peniko::ImageQuality::Low => tiny_skia::FilterQuality::Nearest,
            peniko::ImageQuality::Medium | peniko::ImageQuality::High => {
                tiny_skia::FilterQuality::Bilinear
            }
        };
        let shader = tiny_skia::Pattern::new(
            scratch.as_ref(),
            spread,
            quality,
            image.sampler.alpha,
            brush_transform.map_or_else(SkiaXform::identity, convert_transform),
        );
        let paint = SkiaPaint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        let transform = self.transform;
        let targets = &mut self.targets;
        let Some(target) = targets.last_mut() else {
            return false;
        };
        let clip = target.clips.last();
        target.pixmap.fill_path(
            path,
            &paint,
            convert_fill_rule(style),
            transform,
            clip,
        );
        true
    }
}

/// Returns `outer` applied after `inner` (`outer` ∘ `inner`), spelled out
/// instead of relying on pre/post-concat argument order. `from_row` takes
/// `(sx, ky, kx, sy, tx, ty)`.
fn concat(outer: SkiaXform, inner: SkiaXform) -> SkiaXform {
    SkiaXform::from_row(
        outer.sx * inner.sx + outer.kx * inner.ky,
        outer.ky * inner.sx + outer.sy * inner.ky,
        outer.sx * inner.kx + outer.kx * inner.sy,
        outer.ky * inner.kx + outer.sy * inner.sy,
        outer.sx * inner.tx + outer.kx * inner.ty + outer.tx,
        outer.ky * inner.tx + outer.sy * inner.ty + outer.ty,
    )
}

/// Converts a `kurbo` affine into a `tiny-skia` transform. `kurbo` stores
/// coefficients `[a, b, c, d, e, f]` as `x' = a*x + c*y + e`,
/// `y' = b*x + d*y + f`; `tiny-skia`'s `from_row` takes
/// `(sx, ky, kx, sy, tx, ty)`, so the coefficients pass through in order.
fn convert_transform(transform: Affine) -> SkiaXform {
    let coeffs = transform.as_coeffs();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page coordinates fit comfortably in f32"
    )]
    SkiaXform::from_row(
        coeffs[0] as f32,
        coeffs[1] as f32,
        coeffs[2] as f32,
        coeffs[3] as f32,
        coeffs[4] as f32,
        coeffs[5] as f32,
    )
}

/// Flattens a `kurbo` shape into a `tiny-skia` path. Returns `None` when the
/// shape is empty.
fn convert_shape(shape: &impl Shape) -> Option<Path> {
    let bezier = shape.into_path(TOLERANCE);
    let mut builder = PathBuilder::new();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "page coordinates fit comfortably in f32"
    )]
    for element in bezier.elements() {
        match element {
            PathEl::MoveTo(point) => builder.move_to(point.x as f32, point.y as f32),
            PathEl::LineTo(point) => builder.line_to(point.x as f32, point.y as f32),
            PathEl::QuadTo(first, second) => {
                builder.quad_to(first.x as f32, first.y as f32, second.x as f32, second.y as f32);
            }
            PathEl::CurveTo(first, second, third) => builder.cubic_to(
                first.x as f32,
                first.y as f32,
                second.x as f32,
                second.y as f32,
                third.x as f32,
                third.y as f32,
            ),
            PathEl::ClosePath => builder.close(),
        }
    }
    builder.finish()
}

/// Converts a `peniko` color to a `tiny-skia` color.
fn convert_color(color: Color) -> tiny_skia::Color {
    let rgba = color.to_rgba8();
    tiny_skia::Color::from_rgba8(rgba.r, rgba.g, rgba.b, rgba.a)
}

/// Converts gradient stops; an empty stop list reads as transparent.
fn convert_stops(stops: &peniko::ColorStops) -> Option<Vec<tiny_skia::GradientStop>> {
    if stops.is_empty() {
        return None;
    }
    Some(
        stops
            .iter()
            .map(|stop| {
                tiny_skia::GradientStop::new(
                    stop.offset,
                    convert_color(stop.color.to_alpha_color::<peniko::color::Srgb>()),
                )
            })
            .collect(),
    )
}

/// Resolves any brush but images to a `tiny-skia` shader. Image brushes
/// draw through [`TinySkiaScenePainter::fill_image`], which needs the
/// scratch pixmap; anywhere else (strokes, glyphs) they read as transparent
/// like `Resource`/`Custom`, which carry no pixels by definition (the
/// `vello_cpu` backend agrees).
fn convert_brush(brush: &PaintRef<'_>, brush_transform: Option<Affine>) -> tiny_skia::Shader<'static> {
    use tiny_skia::Shader;
    match brush {
        Paint::Solid(color) => Shader::SolidColor(convert_color(*color)),
        Paint::Gradient(gradient) => convert_gradient(gradient, brush_transform)
            .unwrap_or(Shader::SolidColor(tiny_skia::Color::TRANSPARENT)),
        Paint::Image(_) | Paint::Resource(_) | Paint::Custom(_) => {
            Shader::SolidColor(tiny_skia::Color::TRANSPARENT)
        }
    }
}

/// Converts linear, radial (two-point conical), and sweep gradients.
/// `tiny-skia` constructors build the shader directly and reject degenerate
/// geometry with `None`. Sweep angles arrive in radians from `peniko` and
/// leave in degrees for `tiny-skia`.
fn convert_gradient(
    gradient: &peniko::Gradient,
    brush_transform: Option<Affine>,
) -> Option<tiny_skia::Shader<'static>> {
    use tiny_skia::{LinearGradient, RadialGradient, SweepGradient};
    let stops = convert_stops(&gradient.stops)?;
    let spread = match gradient.extend {
        peniko::Extend::Pad => SkiaSpread::Pad,
        peniko::Extend::Repeat => SkiaSpread::Repeat,
        peniko::Extend::Reflect => SkiaSpread::Reflect,
    };
    let transform = brush_transform.map_or_else(SkiaXform::identity, convert_transform);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "gradient geometry fits comfortably in f32"
    )]
    match gradient.kind {
        peniko::GradientKind::Linear(peniko::LinearGradientPosition { start, end }) => {
            LinearGradient::new(
                tiny_skia::Point::from_xy(start.x as f32, start.y as f32),
                tiny_skia::Point::from_xy(end.x as f32, end.y as f32),
                stops,
                spread,
                transform,
            )
        }
        peniko::GradientKind::Radial(peniko::RadialGradientPosition {
            start_center,
            start_radius,
            end_center,
            end_radius,
        }) => RadialGradient::new(
            tiny_skia::Point::from_xy(start_center.x as f32, start_center.y as f32),
            start_radius,
            tiny_skia::Point::from_xy(end_center.x as f32, end_center.y as f32),
            end_radius,
            stops,
            spread,
            transform,
        ),
        peniko::GradientKind::Sweep(peniko::SweepGradientPosition {
            center,
            start_angle,
            end_angle,
        }) => SweepGradient::new(
            tiny_skia::Point::from_xy(center.x as f32, center.y as f32),
            start_angle.to_degrees(),
            end_angle.to_degrees(),
            stops,
            spread,
            transform,
        ),
    }
}

/// Converts a `peniko` fill rule.
fn convert_fill_rule(rule: Fill) -> SkiaRule {
    match rule {
        Fill::NonZero => SkiaRule::Winding,
        Fill::EvenOdd => SkiaRule::EvenOdd,
    }
}

/// Converts a `kurbo` stroke style. A dash list that fails to build drops
/// the dash but keeps the stroke.
fn convert_stroke(style: &Stroke) -> SkiaStroke {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "stroke widths fit comfortably in f32"
    )]
    let mut stroke = SkiaStroke {
        width: style.width as f32,
        miter_limit: style.miter_limit as f32,
        line_cap: match (style.start_cap, style.end_cap) {
            (kurbo::Cap::Butt, kurbo::Cap::Butt) => tiny_skia::LineCap::Butt,
            (kurbo::Cap::Square, kurbo::Cap::Square) => tiny_skia::LineCap::Square,
            _ => tiny_skia::LineCap::Round,
        },
        line_join: match style.join {
            kurbo::Join::Miter => tiny_skia::LineJoin::Miter,
            kurbo::Join::Round => tiny_skia::LineJoin::Round,
            kurbo::Join::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: None,
    };
    if !style.dash_pattern.is_empty() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "dash lengths fit comfortably in f32"
        )]
        if let Some(dash) = SkiaDash::new(
            style.dash_pattern.iter().map(|dash| *dash as f32).collect(),
            style.dash_offset as f32,
        ) {
            stroke.dash = Some(dash);
        }
    }
    stroke
}

/// Converts the common blend modes; anything exotic composites source-over.
fn convert_blend(mode: BlendMode) -> SkiaBlend {
    use peniko::{Compose, Mix};
    match mode.compose {
        Compose::Clear => SkiaBlend::Clear,
        Compose::Copy => SkiaBlend::Source,
        Compose::SrcIn => SkiaBlend::SourceIn,
        Compose::DestIn => SkiaBlend::DestinationIn,
        _ => match mode.mix {
            Mix::Multiply => SkiaBlend::Multiply,
            Mix::Screen => SkiaBlend::Screen,
            Mix::Overlay => SkiaBlend::Overlay,
            Mix::Darken => SkiaBlend::Darken,
            Mix::Lighten => SkiaBlend::Lighten,
            _ => SkiaBlend::SourceOver,
        },
    }
}

/// Builds the mask for one clip push: the parent mask when there is one,
/// intersected with the new path over a white base otherwise.
fn push_mask(
    width: u32,
    height: u32,
    parent: Option<&Mask>,
    shape: &impl Shape,
    transform: Affine,
) -> Option<Mask> {
    let path = convert_shape(shape)?;
    let blank = || {
        Mask::from_vec(
            vec![255; width as usize * height as usize],
            IntSize::from_wh(width, height)?,
        )
    };
    let mut mask = parent.cloned().or_else(blank)?;
    mask.intersect_path(&path, SkiaRule::Winding, true, convert_transform(transform));
    Some(mask)
}

/// Converts decoded image bytes into a premultiplied pixmap. Blitz hands
/// straight-alpha `Rgba8` (and occasionally premultiplied or `Bgra8`);
/// anything else, or a length mismatch, reads as transparent.
fn image_to_pixmap(
    data: &[u8],
    format: peniko::ImageFormat,
    alpha: peniko::ImageAlphaType,
    width: u32,
    height: u32,
) -> Option<Pixmap> {
    use peniko::{ImageAlphaType, ImageFormat};
    let pixels = usize::try_from(width).ok()?.checked_mul(usize::try_from(height).ok()?)?;
    let len = pixels.checked_mul(4)?;
    if data.len() != len {
        return None;
    }
    if alpha == ImageAlphaType::AlphaPremultiplied && format == ImageFormat::Rgba8 {
        return Pixmap::from_vec(data.to_vec(), IntSize::from_wh(width, height)?);
    }
    let mut out = Vec::with_capacity(len);
    for pixel in data.as_chunks::<4>().0 {
        let (red, green, blue, byte_alpha) = (pixel[0], pixel[1], pixel[2], pixel[3]);
        let (red, blue) = if format == ImageFormat::Bgra8 {
            (blue, red)
        } else {
            (red, blue)
        };
        if alpha == ImageAlphaType::AlphaPremultiplied {
            out.extend_from_slice(&[red, green, blue, byte_alpha]);
        } else {
            // Same rounding as a PNG decode: `(c*a+127)/255`, at most 255.
            let mix = |channel: u8| {
                u8::try_from((u16::from(channel) * u16::from(byte_alpha) + 127) / 255)
                    .unwrap_or(u8::MAX)
            };
            out.extend_from_slice(&[mix(red), mix(green), mix(blue), byte_alpha]);
        }
    }
    Pixmap::from_vec(out, IntSize::from_wh(width, height)?)
}

/// Collects one glyph outline, flipping font y-up coordinates to screen
/// y-down around the origin; the caller places it with the pen transform.
struct GlyphPen {
    builder: PathBuilder,
}

impl GlyphPen {
    fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
        }
    }

    fn finish(self) -> Option<Path> {
        self.builder.finish()
    }
}

impl skrifa::outline::OutlinePen for GlyphPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder.move_to(x, -y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder.line_to(x, -y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(cx0, -cy0, x, -y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(cx0, -cy0, cx1, -cy1, x, -y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

impl RenderContext for TinySkiaScenePainter {}

impl PaintScene for TinySkiaScenePainter {
    fn reset(&mut self) {
        let (width, height) = {
            let canvas = &self.targets[0].pixmap;
            (canvas.width(), canvas.height())
        };
        self.targets.clear();
        self.targets.push(Target {
            pixmap: make_pixmap(width, height),
            paint: PixmapPaint::default(),
            clips: Vec::new(),
        });
        self.image_scratch = None;
        self.transform = SkiaXform::identity();
    }

    fn push_layer(
        &mut self,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        filter: Option<Arc<Filter>>,
        _backdrop_filter: Option<Arc<Filter>>,
    ) {
        // Filters run on pop in the blur unit; backdrops are ignored like the
        // vello_cpu backend ignores them.
        let _ = filter;
        self.transform = convert_transform(transform);
        let (width, height) = {
            let canvas = &self.targets[0].pixmap;
            (canvas.width(), canvas.height())
        };
        let parent = self
            .targets
            .last()
            .and_then(|target| target.clips.last());
        let mask = push_mask(width, height, parent, clip, transform);
        let paint = PixmapPaint {
            opacity: alpha.clamp(0.0, 1.0),
            blend_mode: convert_blend(blend.into()),
            ..Default::default()
        };
        self.targets.push(Target {
            pixmap: make_pixmap(width, height),
            paint,
            clips: mask.map_or_else(Vec::new, |mask| vec![mask]),
        });
    }

    fn push_clip_layer(&mut self, transform: Affine, clip: &impl Shape) {
        self.transform = convert_transform(transform);
        let (width, height) = {
            let canvas = &self.targets[0].pixmap;
            (canvas.width(), canvas.height())
        };
        let parent = self
            .targets
            .last()
            .and_then(|target| target.clips.last());
        let Some(mask) = push_mask(width, height, parent, clip, transform) else {
            return;
        };
        if let Some(target) = self.targets.last_mut() {
            target.clips.push(mask);
        }
    }

    fn pop_layer(&mut self) {
        if self.targets.len() <= 1 {
            return;
        }
        let Some(layer) = self.targets.pop() else {
            return;
        };
        let paint = layer.paint;
        let Some(target) = self.targets.last_mut() else {
            return;
        };
        target.pixmap.draw_pixmap(
            0,
            0,
            layer.pixmap.as_ref(),
            &paint,
            SkiaXform::identity(),
            None,
        );
    }


    fn stroke<'a>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.transform = convert_transform(transform);
        let Some(path) = convert_shape(shape) else {
            return;
        };
        let shader = convert_brush(&brush.into(), brush_transform);
        let paint = SkiaPaint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        let transform = self.transform;
        let (target, clip) = self.draw_target();
        target.stroke_path(&path, &paint, &convert_stroke(style), transform, clip);
    }

    fn fill<'a>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        self.transform = convert_transform(transform);
        let Some(path) = convert_shape(shape) else {
            return;
        };
        let brush: PaintRef<'_> = brush.into();
        // Image brushes convert through the scratch pixmap; anything else
        // resolves to a plain shader below.
        if let Paint::Image(image) = &brush
            && self.fill_image(image, brush_transform, style, &path)
        {
            return;
        }
        let shader = convert_brush(&brush, brush_transform);
        let paint = SkiaPaint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        let transform = self.transform;
        let (target, clip) = self.draw_target();
        target.fill_path(&path, &paint, convert_fill_rule(style), transform, clip);
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        _hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        _embolden: kurbo::Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        _brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        // `tiny-skia` has no hinter, so outlines always rasterize
        // anti-aliased. Synthetic emboldening only matters when no bold face
        // exists; the Chromium pixel-diff pass will catch it if it regresses.
        use skrifa::outline::DrawSettings;
        use skrifa::{FontRef, MetadataProvider as _};

        self.transform = convert_transform(transform);
        let data = font.data.data();
        let Ok(font) = FontRef::from_index(data, font.index) else {
            return;
        };
        let outlines = font.outline_glyphs();
        // `peniko` coords are 2.14 fixed point, matching `skrifa`'s
        // `F2Dot14`, so the raw bits transfer directly.
        let coords: Vec<skrifa::instance::NormalizedCoord> = normalized_coords
            .iter()
            .map(|coord| skrifa::instance::NormalizedCoord::from_bits(*coord))
            .collect();
        // `DrawSettings` scales outlines to pixel size itself, so glyphs only
        // need placing at their pen position.
        let location = skrifa::instance::LocationRef::from(coords.as_slice());

        let shader = convert_brush(&brush.into(), None);
        let paint = SkiaPaint {
            shader,
            anti_alias: true,
            ..Default::default()
        };
        let stroked = match style.into() {
            StyleRef::Fill(_) => None,
            StyleRef::Stroke(stroke) => Some(convert_stroke(stroke)),
        };

        for glyph in glyphs {
            let Some(outline) = outlines.get(skrifa::GlyphId::new(glyph.id)) else {
                continue;
            };
            let mut pen = GlyphPen::new();
            let settings =
                DrawSettings::unhinted(skrifa::instance::Size::new(font_size), location);
            if outline.draw(settings, &mut pen).is_err() {
                continue;
            }
            let Some(path) = pen.finish() else {
                continue;
            };
            let placed = concat(
                self.transform,
                concat(
                    glyph_transform.map_or_else(SkiaXform::identity, convert_transform),
                    SkiaXform::from_translate(glyph.x, glyph.y),
                ),
            );
            let (target, clip) = self.draw_target();
            if let Some(stroke) = &stroked {
                target.stroke_path(&path, &paint, stroke, placed, clip);
            } else {
                target.fill_path(&path, &paint, SkiaRule::Winding, placed, clip);
            }
        }
    }

    fn draw_box_shadow(
        &mut self,
        transform: Affine,
        rect: kurbo::Rect,
        color: Color,
        _radius: f64,
        _std_dev: f64,
    ) {
        // Sharp until the blur unit softens it: the old in-tree painter never
        // drew shadows at all, so this is already ahead of `main`.
        self.transform = convert_transform(transform);
        let Some(path) = convert_shape(&rect) else {
            return;
        };
        let paint = SkiaPaint {
            shader: tiny_skia::Shader::SolidColor(convert_color(color)),
            anti_alias: true,
            ..Default::default()
        };
        let transform = self.transform;
        let (target, clip) = self.draw_target();
        target.fill_path(&path, &paint, SkiaRule::Winding, transform, clip);
    }
}

#[cfg(test)]
mod tests {
    use super::TinySkiaScenePainter;
    use anyrender::PaintScene as _;
    use kurbo::{Affine, Rect};
    use peniko::Fill;
    use peniko::color::palette::css::{BLUE, RED};

    fn pixel(painter: &TinySkiaScenePainter, x: u32, y: u32) -> [u8; 4] {
        let pixel = painter
            .pixmap()
            .pixel(x, y)
            .expect("test pixel inside canvas");
        [pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()]
    }

    #[test]
    fn solid_fill_paints_rect() {
        let mut painter = TinySkiaScenePainter::new(40, 40);
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(10.0, 10.0, 20.0, 20.0),
        );
        assert_eq!(pixel(&painter, 15, 15), [255, 0, 0, 255]);
        assert_eq!(pixel(&painter, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&painter, 25, 25), [0, 0, 0, 0]);
    }

    #[test]
    fn linear_gradient_runs_red_to_blue() {
        use peniko::color::DynamicColor;
        use peniko::{ColorStops, Extend, Gradient, GradientKind};
        let mut stops = ColorStops::new();
        stops.push(peniko::ColorStop {
            offset: 0.0,
            color: DynamicColor::from(RED),
        });
        stops.push(peniko::ColorStop {
            offset: 1.0,
            color: DynamicColor::from(BLUE),
        });
        let gradient = Gradient {
            kind: GradientKind::Linear(
                peniko::LinearGradientPosition::new(
                    kurbo::Point::new(0.0, 0.0),
                    kurbo::Point::new(40.0, 0.0),
                ),
            ),
            extend: Extend::Pad,
            stops,
            ..Default::default()
        };
        let mut painter = TinySkiaScenePainter::new(40, 10);
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &gradient,
            None,
            &Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        let left = pixel(&painter, 2, 5);
        let right = pixel(&painter, 37, 5);
        assert!(left[0] > 200 && left[3] == 255, "left is red: {left:?}");
        assert!(right[2] > 200 && right[3] == 255, "right is blue: {right:?}");
    }

    #[test]
    fn clip_layer_masks_content() {
        let mut painter = TinySkiaScenePainter::new(40, 40);
        painter.push_clip_layer(Affine::IDENTITY, &Rect::new(0.0, 0.0, 10.0, 40.0));
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 40.0, 40.0),
        );
        painter.pop_layer();
        assert_eq!(pixel(&painter, 5, 20), [255, 0, 0, 255]);
        assert_eq!(pixel(&painter, 20, 20), [0, 0, 0, 0]);
    }

    #[test]
    fn blend_layer_applies_alpha() {
        let mut painter = TinySkiaScenePainter::new(40, 40);
        painter.push_layer(
            peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
            0.5,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 40.0, 40.0),
            None,
            None,
        );
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 40.0, 40.0),
        );
        painter.pop_layer();
        let middle = pixel(&painter, 20, 20);
        assert!(
            (100..=160).contains(&middle[0]) && (100..=160).contains(&middle[3]),
            "half red over transparent: {middle:?}"
        );
    }

    #[test]
    fn stroke_outlines_rect() {
        let mut painter = TinySkiaScenePainter::new(40, 40);
        let stroke = kurbo::Stroke::new(4.0);
        painter.stroke(
            &stroke,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(10.0, 10.0, 30.0, 30.0),
        );
        assert_eq!(pixel(&painter, 10, 20), [255, 0, 0, 255]);
        assert_eq!(pixel(&painter, 20, 20), [0, 0, 0, 0]);
    }

    fn image_brush(
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        format: peniko::ImageFormat,
    ) -> peniko::ImageBrush {
        peniko::ImageBrush {
            image: peniko::ImageData {
                data: peniko::Blob::new(std::sync::Arc::new(pixels)),
                format,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width,
                height,
            },
            sampler: peniko::ImageSampler {
                x_extend: peniko::Extend::Repeat,
                y_extend: peniko::Extend::Repeat,
                quality: peniko::ImageQuality::Medium,
                alpha: 1.0,
            },
        }
    }

    #[test]
    fn straight_image_premultiplies() {
        let brush = image_brush(
            vec![255, 0, 0, 255, 0, 0, 255, 255, 0, 255, 0, 255, 255, 255, 255, 255],
            2,
            2,
            peniko::ImageFormat::Rgba8,
        );
        let mut painter = TinySkiaScenePainter::new(4, 4);
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            brush.as_ref(),
            None,
            &Rect::new(0.0, 0.0, 4.0, 4.0),
        );
        assert_eq!(pixel(&painter, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&painter, 1, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(&painter, 0, 1), [0, 255, 0, 255]);
        assert_eq!(pixel(&painter, 1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn bgra_image_swaps_channels() {
        let brush = image_brush(
            vec![255, 0, 0, 255],
            1,
            1,
            peniko::ImageFormat::Bgra8,
        );
        let mut painter = TinySkiaScenePainter::new(2, 2);
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            brush.as_ref(),
            None,
            &Rect::new(0.0, 0.0, 2.0, 2.0),
        );
        assert_eq!(pixel(&painter, 0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn short_image_reads_transparent() {
        let brush = image_brush(vec![1, 2, 3], 1, 1, peniko::ImageFormat::Rgba8);
        let mut painter = TinySkiaScenePainter::new(2, 2);
        painter.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            brush.as_ref(),
            None,
            &Rect::new(0.0, 0.0, 2.0, 2.0),
        );
        assert_eq!(pixel(&painter, 0, 0), [0, 0, 0, 0]);
    }
}


