//! Canvas 2D context: JS bindings over [`Canvas2D`].
//!
//! The context is a plain platform object, not a node wrapper, so the
//! generated IDL traits (which dispatch on `JsNode`) do not apply. Members
//! are installed on the `CanvasRenderingContext2D` prototype and reach state
//! through the hidden `__tbDoc`/`__tbNode` pair stamped on the instance.
//!
//! Spec: <https://html.spec.whatwg.org/multipage/canvas.html#2dcontext>

#![expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs passes every JS argument by value"
)]

mod color;
mod state;

pub(crate) use state::{Canvas2D, SourceImage, StyleValue};

use rquickjs::function::This;
use rquickjs::object::Accessor;
use rquickjs::prelude::{Func, Opt};
use rquickjs::{Coerced, Ctx, Exception, FromJs, Function, Object, Result, TypedArray, Value};

use crate::dom::tree::NodeId;
use crate::js::bindings::{world, wrap_node};
use state::serialize_color;

/// A `WebIDL` `DOMString` argument: coerces like `ToString`, then owns the
/// UTF-8 bytes for the duration of the call.
pub(crate) struct DomString(std::string::String);

impl std::ops::Deref for DomString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'js> FromJs<'js> for DomString {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        Ok(Self(
            Coerced::<rquickjs::String>::from_js(ctx, value)?.to_string()?,
        ))
    }
}

/// Builds the `CanvasRenderingContext2D` constructor and its prototype.
pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let ctor = Function::new(ctx.clone(), || {})?;
    let prototype = Object::new(ctx.clone())?;
    install_members(ctx, &prototype)?;
    ctor.set("prototype", prototype)?;
    ctx.globals().set("CanvasRenderingContext2D", ctor)?;
    install_image_data(ctx)?;
    Ok(())
}

/// One 2D context object for `id`.
pub(crate) fn context_object<'js>(ctx: &Ctx<'js>, id: NodeId) -> Result<Value<'js>> {
    let ctor: Function = ctx.globals().get("CanvasRenderingContext2D")?;
    let prototype: Object = ctor.get("prototype")?;
    let object = Object::new(ctx.clone())?;
    object.set_prototype(Some(&prototype))?;
    object.set("__tbDoc", id.document)?;
    object.set("__tbNode", id.node.as_u64())?;
    object.set("canvas", wrap_node(ctx, id)?)?;
    let resources = Object::new(ctx.clone())?;
    object.set("__tbRes", resources)?;
    Ok(object.into_value())
}

fn install_members<'js>(ctx: &Ctx<'js>, prototype: &Object<'js>) -> Result<()> {
    install_methods(ctx, prototype)?;
    install_accessors(prototype)
}

fn install_methods<'js>(ctx: &Ctx<'js>, prototype: &Object<'js>) -> Result<()> {
    macro_rules! method {
        ($name:literal, $length:expr, $function:expr) => {
            method(ctx, prototype, $name, $length, $function)?;
        };
    }

    method!("save", 0, save);
    method!("restore", 0, restore);
    method!("reset", 0, reset);
    method!("scale", 2, scale);
    method!("rotate", 1, rotate);
    method!("translate", 2, translate);
    method!("transform", 6, transform);
    method!("setTransform", 0, set_transform);
    method!("resetTransform", 0, reset_transform);
    method!("getTransform", 0, get_transform);

    method!("beginPath", 0, begin_path);
    method!("closePath", 0, close_path);
    method!("moveTo", 2, move_to);
    method!("lineTo", 2, line_to);
    method!("quadraticCurveTo", 4, quadratic_curve_to);
    method!("bezierCurveTo", 6, bezier_curve_to);
    method!("arc", 5, arc);
    method!("arcTo", 5, arc_to);
    method!("ellipse", 7, ellipse);
    method!("rect", 4, rect);
    method!("roundRect", 3, round_rect);

    method!("fill", 0, fill);
    method!("stroke", 0, stroke);
    method!("clip", 0, clip);
    method!("isPointInPath", 2, is_point_in_path);
    method!("isPointInStroke", 2, is_point_in_stroke);
    method!("fillRect", 4, fill_rect);
    method!("strokeRect", 4, stroke_rect);
    method!("clearRect", 4, clear_rect);

    method!("getLineDash", 0, get_line_dash);
    method!("setLineDash", 1, set_line_dash);

    method!("createLinearGradient", 4, create_linear_gradient);
    method!("createRadialGradient", 6, create_radial_gradient);
    method!("createConicGradient", 2, create_conic_gradient);
    method!("createPattern", 2, create_pattern);

    method!("getImageData", 4, get_image_data);
    method!("putImageData", 3, put_image_data);
    method!("createImageData", 2, create_image_data);

    method!("drawImage", 3, draw_image);

    method!("fillText", 3, fill_text);
    method!("strokeText", 3, stroke_text);
    method!("measureText", 1, measure_text);
    method!("drawFocusIfNeeded", 1, noop);
    method!("scrollPathIntoView", 0, noop);

    Ok(())
}

macro_rules! number_prop {
    ($prototype:expr, $name:literal, $get:ident, $set:ident) => {
        $prototype.prop(
            $name,
            Accessor::new(
                |this: This<Object<'_>>| with_canvas(this.0.ctx(), &this.0, |canvas| canvas.$get()),
                |this: This<Object<'_>>, value: Coerced<f64>| {
                    with_canvas(this.0.ctx(), &this.0, |canvas| {
                        canvas.$set(f32_of(*value));
                    })
                },
            ),
        )?;
    };
}

macro_rules! string_prop {
    ($prototype:expr, $name:literal, $get:ident, $set:ident) => {
        $prototype.prop(
            $name,
            Accessor::new(
                |this: This<Object<'_>>| {
                    with_canvas(this.0.ctx(), &this.0, |canvas| canvas.$get().to_owned())
                },
                |this: This<Object<'_>>, value: DomString| {
                    with_canvas(this.0.ctx(), &this.0, |canvas| canvas.$set(&value))
                },
            ),
        )?;
    };
}
fn install_accessors(prototype: &Object<'_>) -> Result<()> {
    install_paint_accessors(prototype)?;
    install_text_accessors(prototype)?;
    prototype.prop(
        "imageSmoothingEnabled",
        Accessor::new(
            |this: This<Object<'_>>| {
                with_canvas(this.0.ctx(), &this.0, |canvas| {
                    canvas.image_smoothing_enabled()
                })
            },
            |this: This<Object<'_>>, value: Coerced<bool>| {
                with_canvas(this.0.ctx(), &this.0, |canvas| {
                    canvas.set_image_smoothing_enabled(*value);
                })
            },
        ),
    )?;
    string_prop!(
        prototype,
        "imageSmoothingQuality",
        image_smoothing_quality,
        set_image_smoothing_quality
    );
    string_prop!(prototype, "filter", filter, set_filter);
    Ok(())
}

fn install_paint_accessors(prototype: &Object<'_>) -> Result<()> {
    prototype.prop("fillStyle", Accessor::new(get_fill_style, set_fill_style))?;
    prototype.prop(
        "strokeStyle",
        Accessor::new(get_stroke_style, set_stroke_style),
    )?;
    number_prop!(prototype, "lineWidth", line_width, set_line_width);
    string_prop!(prototype, "lineCap", line_cap, set_line_cap);
    string_prop!(prototype, "lineJoin", line_join, set_line_join);
    number_prop!(prototype, "miterLimit", miter_limit, set_miter_limit);
    number_prop!(
        prototype,
        "lineDashOffset",
        line_dash_offset,
        set_line_dash_offset
    );
    number_prop!(prototype, "globalAlpha", global_alpha, set_global_alpha);
    string_prop!(
        prototype,
        "globalCompositeOperation",
        global_composite_operation,
        set_global_composite_operation
    );
    number_prop!(prototype, "shadowBlur", shadow_blur, set_shadow_blur);
    prototype.prop(
        "shadowColor",
        Accessor::new(
            |this: This<Object<'_>>| {
                with_canvas(this.0.ctx(), &this.0, |canvas| {
                    serialize_color(canvas.shadow_color())
                })
            },
            |this: This<Object<'_>>, value: DomString| {
                let color = css_color(this.0.ctx(), &this.0, &value)?;
                with_canvas(this.0.ctx(), &this.0, |canvas| {
                    if let Some(color) = color {
                        canvas.set_shadow_color(color);
                    }
                })
            },
        ),
    )?;
    number_prop!(
        prototype,
        "shadowOffsetX",
        shadow_offset_x,
        set_shadow_offset_x
    );
    number_prop!(
        prototype,
        "shadowOffsetY",
        shadow_offset_y,
        set_shadow_offset_y
    );
    Ok(())
}

fn install_text_accessors(prototype: &Object<'_>) -> Result<()> {
    string_prop!(prototype, "font", font, set_font);
    string_prop!(prototype, "textAlign", text_align, set_text_align);
    string_prop!(prototype, "textBaseline", text_baseline, set_text_baseline);
    string_prop!(prototype, "direction", direction, set_direction);
    string_prop!(
        prototype,
        "letterSpacing",
        letter_spacing,
        set_letter_spacing
    );
    string_prop!(prototype, "wordSpacing", word_spacing, set_word_spacing);
    string_prop!(prototype, "fontKerning", font_kerning, set_font_kerning);
    string_prop!(prototype, "fontStretch", font_stretch, set_font_stretch);
    string_prop!(
        prototype,
        "fontVariantCaps",
        font_variant_caps,
        set_font_variant_caps
    );
    Ok(())
}

fn method<'js, F, A>(
    ctx: &Ctx<'js>,
    prototype: &Object<'js>,
    name: &str,
    length: usize,
    function: F,
) -> Result<()>
where
    F: rquickjs::function::IntoJsFunc<'js, A> + 'js,
{
    let function = Function::new(ctx.clone(), function)?
        .with_name(name)?
        .with_length(length)?;
    prototype.set(name, function)?;
    Ok(())
}

#[expect(clippy::unnecessary_wraps, reason = "rquickjs callbacks return Result")]
fn noop(_: This<Object<'_>>) -> Result<()> {
    Ok(())
}

// ---- state access --------------------------------------------------------

fn canvas_id(object: &Object<'_>) -> Result<NodeId> {
    let document: u32 = object.get("__tbDoc")?;
    let node: u64 = object.get("__tbNode")?;
    Ok(NodeId {
        document,
        node: blitz_traits::node_id::NodeId::from_u64(node),
    })
}

fn with_canvas<R>(
    ctx: &Ctx<'_>,
    object: &Object<'_>,
    run: impl FnOnce(&mut Canvas2D) -> R,
) -> Result<R> {
    let id = canvas_id(object)?;
    // `__tbDoc` and `__tbNode` are writable, so they carry a claim, not
    // proof. Require a document this frame created and a real HTML canvas;
    // otherwise a page could retarget another frame's canvas, or any element,
    // in the shared registry.
    let world = world(ctx)?;
    {
        let owner = world.borrow();
        let is_canvas = owner
            .with_document(id, |parsed| {
                parsed
                    .document
                    .base
                    .get_node(id.node)
                    .is_some_and(|node| {
                        crate::dom::tree::is_html_tag(Some(&node.data), "canvas")
                    })
            })
            .unwrap_or(false);
        if !owner.owns_node(id) || !is_canvas {
            return Err(crate::js::bindings::throw_dom(
                ctx,
                "InvalidStateError",
                "canvas does not belong to this frame",
            ));
        }
    }
    let width = canvas_dimension(ctx, id, "width", 300);
    let height = canvas_dimension(ctx, id, "height", 150);
    let registry = world.borrow().registry();
    let mut registry = registry.borrow_mut();
    let canvas = registry.canvas(id, width, height);
    Ok(run(canvas))
}

/// A canvas content attribute as a bitmap dimension
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-canvas-width>).
pub(crate) fn canvas_dimension(ctx: &Ctx<'_>, id: NodeId, attribute: &str, default: u32) -> u32 {
    let value = world(ctx)
        .ok()
        .and_then(|owner| {
            owner.borrow().with_document(id, |parsed| {
                crate::dom::tree::attr(&parsed.document.base, id.node, attribute).map(str::to_owned)
            })
        })
        .flatten()
        .unwrap_or_default();
    crate::dom::parse::parse_non_negative_integer(&value).unwrap_or(default)
}

fn f32_of(value: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "canvas rasterizes coordinates at f32 precision, like Skia"
    )]
    let value = value as f32;
    value
}

fn i32_of(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is finite and clamped by the IDL long conversion below"
    )]
    let value = value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    value
}

// ---- state methods -------------------------------------------------------

fn save(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::save)
}

fn restore(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::restore)
}

fn reset(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::reset)
}

fn scale(this: This<Object<'_>>, x: Coerced<f64>, y: Coerced<f64>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.scale(f32_of(*x), f32_of(*y));
    })
}

fn rotate(this: This<Object<'_>>, angle: Coerced<f64>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.rotate(f32_of(*angle));
    })
}

fn translate(this: This<Object<'_>>, x: Coerced<f64>, y: Coerced<f64>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.translate(f32_of(*x), f32_of(*y));
    })
}

fn transform(
    this: This<Object<'_>>,
    m11: Coerced<f64>,
    m12: Coerced<f64>,
    m21: Coerced<f64>,
    m22: Coerced<f64>,
    dx: Coerced<f64>,
    dy: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.transform(
            f32_of(*m11),
            f32_of(*m12),
            f32_of(*m21),
            f32_of(*m22),
            f32_of(*dx),
            f32_of(*dy),
        );
    })
}

fn set_transform(
    this: This<Object<'_>>,
    m11: Opt<Coerced<f64>>,
    m12: Opt<Coerced<f64>>,
    m21: Opt<Coerced<f64>>,
    m22: Opt<Coerced<f64>>,
    dx: Opt<Coerced<f64>>,
    dy: Opt<Coerced<f64>>,
) -> Result<()> {
    let args = (
        m11.0.as_ref(),
        m12.0.as_ref(),
        m21.0.as_ref(),
        m22.0.as_ref(),
        dx.0.as_ref(),
        dy.0.as_ref(),
    );
    let (Some(m11), Some(m12), Some(m21), Some(m22), Some(dx), Some(dy)) = args else {
        // The one-argument overload takes a dictionary; with no members the
        // transform resets to the identity
        // (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-settransform>).
        if args == (None, None, None, None, None, None) {
            return with_canvas(this.0.ctx(), &this.0, state::Canvas2D::reset_transform);
        }
        return Ok(());
    };
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.set_transform(
            f32_of(**m11),
            f32_of(**m12),
            f32_of(**m21),
            f32_of(**m22),
            f32_of(**dx),
            f32_of(**dy),
        );
    })
}

fn reset_transform(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::reset_transform)
}

fn get_transform(this: This<Object<'_>>) -> Result<Object<'_>> {
    let transform = with_canvas(this.0.ctx(), &this.0, |canvas| canvas.get_transform())?;
    let object = Object::new(this.0.ctx().clone())?;
    object.set("a", transform.sx)?;
    object.set("b", transform.ky)?;
    object.set("c", transform.kx)?;
    object.set("d", transform.sy)?;
    object.set("e", transform.tx)?;
    object.set("f", transform.ty)?;
    object.set("is2D", true)?;
    object.set("isIdentity", transform.is_identity())?;
    Ok(object)
}

// ---- path methods --------------------------------------------------------

fn begin_path(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::begin_path)
}

fn close_path(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::close_path)
}

fn move_to(this: This<Object<'_>>, x: Coerced<f64>, y: Coerced<f64>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.move_to(f32_of(*x), f32_of(*y));
    })
}

fn line_to(this: This<Object<'_>>, x: Coerced<f64>, y: Coerced<f64>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.line_to(f32_of(*x), f32_of(*y));
    })
}

fn quadratic_curve_to(
    this: This<Object<'_>>,
    cx: Coerced<f64>,
    cy: Coerced<f64>,
    x: Coerced<f64>,
    y: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.quadratic_to(f32_of(*cx), f32_of(*cy), f32_of(*x), f32_of(*y));
    })
}

#[allow(clippy::too_many_arguments, reason = "matches the IDL member")]
fn bezier_curve_to(
    this: This<Object<'_>>,
    c1x: Coerced<f64>,
    c1y: Coerced<f64>,
    c2x: Coerced<f64>,
    c2y: Coerced<f64>,
    x: Coerced<f64>,
    y: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.cubic_to(
            f32_of(*c1x),
            f32_of(*c1y),
            f32_of(*c2x),
            f32_of(*c2y),
            f32_of(*x),
            f32_of(*y),
        );
    })
}

fn arc(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    radius: Coerced<f64>,
    start: Coerced<f64>,
    end: Coerced<f64>,
    counterclockwise: Opt<Coerced<bool>>,
) -> Result<()> {
    let counterclockwise = counterclockwise.0.is_some_and(|value| *value);
    if !(*x).is_finite()
        || !(*y).is_finite()
        || !(*radius).is_finite()
        || !(*start).is_finite()
        || !(*end).is_finite()
    {
        return Ok(());
    }
    if *radius < 0.0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "radius is negative",
        ));
    }
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.arc(
            f32_of(*x),
            f32_of(*y),
            f32_of(*radius),
            f32_of(*start),
            f32_of(*end),
            counterclockwise,
        );
    })
}

fn arc_to(
    this: This<Object<'_>>,
    x1: Coerced<f64>,
    y1: Coerced<f64>,
    x2: Coerced<f64>,
    y2: Coerced<f64>,
    radius: Coerced<f64>,
) -> Result<()> {
    if !(*x1).is_finite()
        || !(*y1).is_finite()
        || !(*x2).is_finite()
        || !(*y2).is_finite()
        || !(*radius).is_finite()
    {
        return Ok(());
    }
    if *radius < 0.0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "radius is negative",
        ));
    }
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.arc_to(
            f32_of(*x1),
            f32_of(*y1),
            f32_of(*x2),
            f32_of(*y2),
            f32_of(*radius),
        );
    })
}

#[allow(clippy::too_many_arguments, reason = "matches the IDL member")]
fn ellipse<'js>(this: This<Object<'js>>, args: rquickjs::function::Rest<Value<'js>>) -> Result<()> {
    let ctx = this.0.ctx();
    let number = |index: usize| -> Result<f64> {
        let value = args
            .0
            .get(index)
            .ok_or_else(|| Exception::throw_type(ctx, "missing argument"))?;
        Ok(*Coerced::<f64>::from_js(ctx, value.clone())?)
    };
    let (x, y, radius_x, radius_y) = (number(0)?, number(1)?, number(2)?, number(3)?);
    let (rotation, start, end) = (number(4)?, number(5)?, number(6)?);
    let counterclockwise = args
        .0
        .get(7)
        .map(|value| Coerced::<bool>::from_js(ctx, value.clone()))
        .transpose()?
        .is_some_and(|value| *value);
    if ![x, y, radius_x, radius_y, rotation, start, end]
        .iter()
        .all(|value| value.is_finite())
    {
        return Ok(());
    }
    if radius_x < 0.0 || radius_y < 0.0 {
        return Err(crate::js::bindings::throw_dom(
            ctx,
            "IndexSizeError",
            "radius is negative",
        ));
    }
    with_canvas(ctx, &this.0, |canvas| {
        canvas.ellipse(
            f32_of(x),
            f32_of(y),
            f32_of(radius_x),
            f32_of(radius_y),
            f32_of(rotation),
            f32_of(start),
            f32_of(end),
            counterclockwise,
        );
    })
}

fn rect(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    width: Coerced<f64>,
    height: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.rect(f32_of(*x), f32_of(*y), f32_of(*width), f32_of(*height));
    })
}

fn round_rect<'js>(
    this: This<Object<'js>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    width: Coerced<f64>,
    height: Coerced<f64>,
    radii: Opt<Value<'js>>,
) -> Result<()> {
    // Step 1 returns before the radii list is validated, so a non-finite
    // rectangle never throws for its corners
    // (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-roundrect>).
    if [*x, *y, *width, *height]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Ok(());
    }
    let radii = match radii.0 {
        Some(value) if !value.is_undefined() => round_rect_radii(&value)?,
        _ => None,
    };
    let radii = if let Some(radii) = radii {
        if radii.is_empty() || radii.len() > 4 {
            return Err(Exception::throw_range(
                this.0.ctx(),
                "Between one and four radii are necessary",
            ));
        }
        if radii
            .iter()
            .any(|(rx, ry)| !rx.is_finite() || !ry.is_finite())
        {
            return Ok(());
        }
        if radii.iter().any(|(rx, ry)| *rx < 0.0 || *ry < 0.0) {
            return Err(Exception::throw_range(this.0.ctx(), "radius is negative"));
        }
        radii
    } else {
        // An omitted radii argument uses one zero radius. It still draws the
        // rectangle and leaves the step-14 subpath at (x, y).
        vec![(0.0, 0.0)]
    };
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.round_rect(
            f32_of(*x),
            f32_of(*y),
            f32_of(*width),
            f32_of(*height),
            &radii,
        );
    })
}

/// Reads the `roundRect` radii argument: one number or `DOMPointInit`, or a
/// sequence of either
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-roundrect>).
fn round_rect_radii(value: &Value<'_>) -> Result<Option<Vec<(f32, f32)>>> {
    let Some(array) = value.as_array() else {
        let Some(pair) = round_rect_radius(value)? else {
            return Ok(None);
        };
        return Ok(Some(vec![pair]));
    };
    let mut radii = Vec::new();
    for item in array.iter::<Value>() {
        let Some(pair) = round_rect_radius(&item?)? else {
            return Ok(None);
        };
        radii.push(pair);
    }
    Ok(Some(radii))
}

/// One radius: a number for both axes, or a `DOMPointInit` dictionary whose
/// missing members read zero.
fn round_rect_radius(value: &Value<'_>) -> Result<Option<(f32, f32)>> {
    if let Some(object) = value.as_object() {
        // WebIDL dictionary conversion: a missing or undefined member is 0,
        // not NaN from ToNumber(undefined).
        let read = |name: &str| -> Result<f32> {
            let member: Value = object.get(name)?;
            if member.is_undefined() || member.is_null() {
                return Ok(0.0);
            }
            let number = Coerced::<f64>::from_js(value.ctx(), member)?;
            Ok(f32_of(*number))
        };
        return Ok(Some((read("x")?, read("y")?)));
    }
    let number = Coerced::<f64>::from_js(value.ctx(), value.clone())?;
    let number = f32_of(*number);
    Ok(Some((number, number)))
}

// ---- painting ------------------------------------------------------------

/// Converts the `CanvasFillRule` enum. `WebIDL` rejects every value other than
/// the two enum strings with `TypeError`, so an unknown string cannot silently
/// fall back to nonzero
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fill>).
fn fill_rule(ctx: &Ctx<'_>, rule: Option<&DomString>) -> Result<bool> {
    match rule.map(|rule| &**rule) {
        None | Some("nonzero") => Ok(false),
        Some("evenodd") => Ok(true),
        Some(_) => Err(Exception::throw_type(ctx, "invalid fill rule")),
    }
}

fn fill(this: This<Object<'_>>, rule: Opt<DomString>) -> Result<()> {
    let even_odd = fill_rule(this.0.ctx(), rule.0.as_ref())?;
    with_canvas(this.0.ctx(), &this.0, |canvas| canvas.fill(even_odd))
}

fn stroke(this: This<Object<'_>>) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, state::Canvas2D::stroke)
}

fn clip(this: This<Object<'_>>, rule: Opt<DomString>) -> Result<()> {
    let even_odd = fill_rule(this.0.ctx(), rule.0.as_ref())?;
    with_canvas(this.0.ctx(), &this.0, |canvas| canvas.clip(even_odd))
}

fn is_point_in_path(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    rule: Opt<DomString>,
) -> Result<bool> {
    let even_odd = fill_rule(this.0.ctx(), rule.0.as_ref())?;
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.is_point_in_path(f32_of(*x), f32_of(*y), even_odd)
    })
}

fn is_point_in_stroke(this: This<Object<'_>>, x: Coerced<f64>, y: Coerced<f64>) -> Result<bool> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.is_point_in_stroke(f32_of(*x), f32_of(*y))
    })
}

fn fill_rect(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    width: Coerced<f64>,
    height: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.fill_rect(f32_of(*x), f32_of(*y), f32_of(*width), f32_of(*height));
    })
}

fn stroke_rect(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    width: Coerced<f64>,
    height: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.stroke_rect(f32_of(*x), f32_of(*y), f32_of(*width), f32_of(*height));
    })
}

fn clear_rect(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    width: Coerced<f64>,
    height: Coerced<f64>,
) -> Result<()> {
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.clear_rect(f32_of(*x), f32_of(*y), f32_of(*width), f32_of(*height));
    })
}

// ---- line dash -----------------------------------------------------------

fn get_line_dash(this: This<Object<'_>>) -> Result<rquickjs::Array<'_>> {
    let dash = with_canvas(this.0.ctx(), &this.0, |canvas| canvas.line_dash())?;
    let array = rquickjs::Array::new(this.0.ctx().clone())?;
    for (index, value) in dash.into_iter().enumerate() {
        array.set(index, value)?;
    }
    Ok(array)
}

fn set_line_dash<'js>(this: This<Object<'js>>, values: Value<'js>) -> Result<()> {
    let Some(array) = values.as_array() else {
        return Ok(());
    };
    let mut dash = Vec::new();
    for value in array.iter::<Coerced<f64>>() {
        let value = f32_of(*value?);
        // An invalid entry rejects the whole sequence and keeps the old one
        // (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-setlinedash>).
        if !value.is_finite() || value < 0.0 {
            return Ok(());
        }
        dash.push(value);
    }
    with_canvas(this.0.ctx(), &this.0, |canvas| canvas.set_line_dash(dash))
}

// ---- resources -----------------------------------------------------------

fn create_linear_gradient(
    this: This<Object<'_>>,
    x0: Coerced<f64>,
    y0: Coerced<f64>,
    x1: Coerced<f64>,
    y1: Coerced<f64>,
) -> Result<Value<'_>> {
    let id = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.create_linear_gradient(f32_of(*x0), f32_of(*y0), f32_of(*x1), f32_of(*y1))
    })?;
    resource_object(&this.0, "Gradient", id)
}

#[allow(clippy::too_many_arguments, reason = "matches the IDL member")]
fn create_radial_gradient(
    this: This<Object<'_>>,
    x0: Coerced<f64>,
    y0: Coerced<f64>,
    r0: Coerced<f64>,
    x1: Coerced<f64>,
    y1: Coerced<f64>,
    r1: Coerced<f64>,
) -> Result<Value<'_>> {
    let id = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.create_radial_gradient(
            f32_of(*x0),
            f32_of(*y0),
            f32_of(*r0),
            f32_of(*x1),
            f32_of(*y1),
            f32_of(*r1),
        )
    })?;
    resource_object(&this.0, "Gradient", id)
}

fn create_conic_gradient(
    this: This<Object<'_>>,
    x: Coerced<f64>,
    y: Coerced<f64>,
    angle: Coerced<f64>,
) -> Result<Value<'_>> {
    let id = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.create_conic_gradient(f32_of(*x), f32_of(*y), f32_of(*angle))
    })?;
    resource_object(&this.0, "Gradient", id)
}

fn create_pattern<'js>(
    this: This<Object<'js>>,
    source: Value<'js>,
    repetition: Opt<DomString>,
) -> Result<Value<'js>> {
    let repetition = repetition
        .0
        .as_ref()
        .map_or_else(|| "repeat".to_owned(), |value| value.to_string());
    let Some(source) = source_image(this.0.ctx(), source)? else {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "InvalidStateError",
            "image is not decoded",
        ));
    };
    let smoothing = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.image_smoothing_enabled()
    })?;
    let id = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.create_pattern(source, &repetition, smoothing)
    })?;
    let Some(id) = id else {
        return Ok(Value::new_null(this.0.ctx().clone()));
    };
    resource_object(&this.0, "Pattern", id)
}

fn resource_object<'js>(this: &Object<'js>, kind: &str, id: u32) -> Result<Value<'js>> {
    let object = Object::new(this.ctx().clone())?;
    object.set("__tbDoc", this.get::<_, u32>("__tbDoc")?)?;
    object.set("__tbNode", this.get::<_, u64>("__tbNode")?)?;
    object.set(format!("__tb{kind}"), id)?;
    if kind == "Gradient" {
        object.set("addColorStop", Func::from(add_color_stop))?;
    }
    let resources: Object = this.get("__tbRes")?;
    resources.set(id.to_string(), object.clone())?;
    Ok(object.into_value())
}

fn add_color_stop(this: This<Object<'_>>, offset: Coerced<f64>, color: DomString) -> Result<()> {
    let id: u32 = this.0.get("__tbGradient")?;
    let Some(color) = color::parse(&color) else {
        return Err(Exception::throw_syntax(
            this.0.ctx(),
            "invalid color for addColorStop",
        ));
    };
    if !(0.0..=1.0).contains(&*offset) {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "offset out of range",
        ));
    }
    let object = this.0.clone();
    let id_wrapper = Object::new(this.0.ctx().clone())?;
    id_wrapper.set("__tbDoc", object.get::<_, u32>("__tbDoc")?)?;
    id_wrapper.set("__tbNode", object.get::<_, u64>("__tbNode")?)?;
    with_canvas(this.0.ctx(), &id_wrapper, |canvas| {
        canvas.add_color_stop(id, f32_of(*offset), color);
    })
}

fn resource_id(object: &Object<'_>, kind: &str) -> Option<u32> {
    object.get::<_, u32>(format!("__tb{kind}")).ok()
}

// ---- image data ----------------------------------------------------------

fn install_image_data(ctx: &Ctx<'_>) -> Result<()> {
    let prototype = Object::new(ctx.clone())?;
    prototype.prop(
        "width",
        Accessor::new(image_data_width, image_data_read_only),
    )?;
    prototype.prop(
        "height",
        Accessor::new(image_data_height, image_data_read_only),
    )?;
    prototype.prop("data", Accessor::new(image_data_data, image_data_read_only))?;
    prototype.prop(
        "colorSpace",
        Accessor::new(image_data_color_space, image_data_read_only),
    )?;
    prototype.prop(
        "pixelFormat",
        Accessor::new(image_data_pixel_format, image_data_read_only),
    )?;
    let ctor =
        rquickjs::function::Constructor::new_prototype(ctx, prototype, image_data_construct)?;
    ctx.globals().set("ImageData", ctor)?;
    Ok(())
}

fn image_data_width(this: This<Object<'_>>) -> Result<u32> {
    this.0.get("__tbWidth")
}

fn image_data_height(this: This<Object<'_>>) -> Result<u32> {
    this.0.get("__tbHeight")
}

fn image_data_data(this: This<Object<'_>>) -> Result<Value<'_>> {
    this.0.get("__tbData")
}

fn image_data_color_space(this: This<Object<'_>>) -> Result<rquickjs::String<'_>> {
    this.0.get("__tbColorSpace")
}

fn image_data_pixel_format(this: This<Object<'_>>) -> Result<rquickjs::String<'_>> {
    this.0.get("__tbPixelFormat")
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "accessor setters use the fallible host signature"
)]
fn image_data_read_only(_this: This<Object<'_>>, _value: Value<'_>) -> Result<()> {
    Ok(())
}

/// `ImageData` constructor, both overloads
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-imagedata-imagedata>).
fn image_data_construct<'js>(
    ctx: Ctx<'js>,
    args: rquickjs::function::Rest<Value<'js>>,
) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    let ctx = &ctx;
    let Some(first) = args.0.first() else {
        return Err(Exception::throw_type(ctx, "ImageData needs arguments"));
    };
    let Some(data) = first
        .as_object()
        .and_then(|object| object.as_typed_array::<rquickjs::U8Clamped>())
    else {
        if first.as_object().is_some() {
            return Err(Exception::throw_type(
                ctx,
                "ImageData data must be a Uint8ClampedArray",
            ));
        }
        let width = *Coerced::<f64>::from_js(ctx, first.clone())?;
        let height = args
            .0
            .get(1)
            .map(|value| Coerced::<f64>::from_js(ctx, value.clone()).map(|value| *value))
            .transpose()?
            .ok_or_else(|| Exception::throw_type(ctx, "ImageData needs a height"))?;
        new_blank_image_data(ctx, &object, width, height, args.0.get(2))?;
        return Ok(object);
    };
    let byte_length = data.len();
    let width = args
        .0
        .get(1)
        .map(|value| Coerced::<f64>::from_js(ctx, value.clone()).map(|value| *value))
        .transpose()?;
    let height = args
        .0
        .get(2)
        .map(|value| Coerced::<f64>::from_js(ctx, value.clone()).map(|value| *value))
        .transpose()?;
    let explicit_dimensions = width.is_some() && height.is_some();
    let (width, height) = match (width, height) {
        (None, _) => (u32::try_from(byte_length / 4).unwrap_or(u32::MAX), 1),
        (Some(width), None) => {
            let width = canvas_u32(width);
            if width == 0 {
                return Err(crate::js::bindings::throw_dom(
                    ctx,
                    "IndexSizeError",
                    "ImageData width must be nonzero",
                ));
            }
            let height = byte_length / 4 / width as usize;
            (width, u32::try_from(height).unwrap_or(u32::MAX))
        }
        (Some(width), Some(height)) => {
            if !width.is_finite() || !height.is_finite() {
                return Err(Exception::throw_type(
                    ctx,
                    "ImageData dimensions must be finite",
                ));
            }
            (canvas_u32(width), canvas_u32(height))
        }
    };
    if width == 0 || (explicit_dimensions && height == 0) {
        return Err(crate::js::bindings::throw_dom(
            ctx,
            "IndexSizeError",
            "ImageData dimensions must be nonzero",
        ));
    }
    if height == 0 || u64::from(width) * u64::from(height) * 4 != byte_length as u64 {
        let name = if explicit_dimensions {
            "IndexSizeError"
        } else {
            "InvalidStateError"
        };
        return Err(crate::js::bindings::throw_dom(
            ctx,
            name,
            "ImageData data length does not match its dimensions",
        ));
    }
    let settings = args.0.get(if args.0.len() > 3 { 3 } else { 2 });
    let (color_space, pixel_format) = image_data_settings(settings);
    set_image_data(
        ctx,
        &object,
        width,
        height,
        first.clone(),
        color_space,
        pixel_format,
    )?;
    Ok(object)
}

fn new_blank_image_data<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    width: f64,
    height: f64,
    settings: Option<&Value<'js>>,
) -> Result<()> {
    if !width.is_finite() || !height.is_finite() {
        return Err(Exception::throw_type(
            ctx,
            "ImageData dimensions must be finite",
        ));
    }
    let width = canvas_u32(width);
    let height = canvas_u32(height);
    if width == 0 || height == 0 {
        return Err(crate::js::bindings::throw_dom(
            ctx,
            "IndexSizeError",
            "ImageData dimensions must be nonzero",
        ));
    }
    let Some(bytes) = crate::js::world::canvas_bytes(width, height) else {
        return Err(crate::js::bindings::throw_dom(
            ctx,
            "IndexSizeError",
            "ImageData area is too large",
        ));
    };
    let data = data_array(ctx, vec![0; bytes])?.into_value();
    let (color_space, pixel_format) = image_data_settings(settings);
    set_image_data(ctx, object, width, height, data, color_space, pixel_format)
}

fn image_data_settings(settings: Option<&Value<'_>>) -> (String, String) {
    let read = |name: &str, default: &str| {
        settings
            .and_then(|value| value.as_object())
            .and_then(|object| object.get::<_, DomString>(name).ok())
            .map_or_else(|| default.to_owned(), |value| value.to_string())
    };
    (
        read("colorSpace", "srgb"),
        read("pixelFormat", "rgba-unorm8"),
    )
}

fn set_image_data<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    width: u32,
    height: u32,
    data: Value<'js>,
    color_space: String,
    pixel_format: String,
) -> Result<()> {
    object.set("__tbWidth", width)?;
    object.set("__tbHeight", height)?;
    object.set("__tbData", data)?;
    object.set(
        "__tbColorSpace",
        rquickjs::String::from_str(ctx.clone(), &color_space)?,
    )?;
    object.set(
        "__tbPixelFormat",
        rquickjs::String::from_str(ctx.clone(), &pixel_format)?,
    )?;
    Ok(())
}

fn data_array<'js>(ctx: &Ctx<'js>, bytes: Vec<u8>) -> Result<TypedArray<'js, rquickjs::U8Clamped>> {
    let bytes = bytes
        .into_iter()
        .map(rquickjs::U8Clamped)
        .collect::<Vec<_>>();
    TypedArray::new(ctx.clone(), bytes)
}

fn get_image_data(
    this: This<Object<'_>>,
    sx: Coerced<f64>,
    sy: Coerced<f64>,
    sw: Coerced<f64>,
    sh: Coerced<f64>,
) -> Result<Value<'_>> {
    if [*sx, *sy, *sw, *sh].iter().any(|value| !value.is_finite()) {
        return Err(Exception::throw_type(
            this.0.ctx(),
            "getImageData arguments must be finite",
        ));
    }
    if *sw == 0.0 || *sh == 0.0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "source width or height is zero",
        ));
    }
    // Negative extents move the origin and normalize, like Blink
    // (`base_rendering_context_2d.cc`, getImageDataInternal).
    let (mut sx, mut sy, mut sw, mut sh) = (*sx, *sy, *sw, *sh);
    if sw < 0.0 {
        sx += sw;
        sw = -sw;
    }
    if sh < 0.0 {
        sy += sh;
        sh = -sh;
    }
    let width = canvas_u32(sw);
    let height = canvas_u32(sh);
    if width == 0 || height == 0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "source width or height rounds to zero",
        ));
    }
    let x = i32_of(sx);
    let y = i32_of(sy);
    let bytes = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.read_pixels(x, y, width, height)
    })?
    .ok_or_else(|| {
        crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "getImageData area is too large",
        )
    })?;
    image_data_object(this.0.ctx(), width, height, bytes)
}

fn put_image_data<'js>(
    this: This<Object<'js>>,
    data: Value<'js>,
    args: rquickjs::function::Rest<Coerced<f64>>,
) -> Result<()> {
    let ctx = this.0.ctx();
    let number = |index: usize| -> Result<f64> {
        let value = args
            .0
            .get(index)
            .ok_or_else(|| Exception::throw_type(ctx, "missing argument"))?;
        Ok(**value)
    };
    let (dx, dy) = (number(0)?, number(1)?);
    if !dx.is_finite() || !dy.is_finite() {
        return Err(Exception::throw_type(
            ctx,
            "putImageData coordinates must be finite",
        ));
    }
    let Some((width, height, pixels)) = read_image_data(&data)? else {
        return Err(Exception::throw_type(ctx, "putImageData expects ImageData"));
    };
    let dirty = match (number(2), number(3), number(4), number(5)) {
        (Ok(x), Ok(y), Ok(width), Ok(height)) => {
            if ![x, y, width, height].iter().all(|value| value.is_finite()) {
                return Err(Exception::throw_type(
                    ctx,
                    "putImageData dirty rectangle must be finite",
                ));
            }
            Some((i32_of(x), i32_of(y), i32_of(width), i32_of(height)))
        }
        _ => None,
    };
    with_canvas(ctx, &this.0, |canvas| {
        canvas.write_pixels(&pixels, width, height, i32_of(dx), i32_of(dy), dirty);
    })
}

fn create_image_data<'js>(
    this: This<Object<'js>>,
    first: Value<'js>,
    second: Opt<Coerced<f64>>,
) -> Result<Value<'js>> {
    let ctx = this.0.ctx();
    canvas_id(&this.0)?;
    if let Some(second) = second.0 {
        let width = *Coerced::<f64>::from_js(ctx, first)?;
        let height = *second;
        if !width.is_finite() || !height.is_finite() {
            return Err(Exception::throw_type(
                ctx,
                "ImageData dimensions must be finite",
            ));
        }
        // Blink takes the absolute value of both dimensions
        // (`BaseRenderingContext2D::createImageData`).
        let width = canvas_u32(width.abs());
        let height = canvas_u32(height.abs());
        if width == 0 || height == 0 {
            return Err(crate::js::bindings::throw_dom(
                ctx,
                "IndexSizeError",
                "ImageData dimensions must be nonzero",
            ));
        }
        let Some(bytes) = crate::js::world::canvas_bytes(width, height) else {
            return Err(crate::js::bindings::throw_dom(
                ctx,
                "IndexSizeError",
                "ImageData area is too large",
            ));
        };
        return image_data_object(ctx, width, height, vec![0; bytes]);
    }
    let Some((width, height, _)) = read_image_data(&first)? else {
        return Err(Exception::throw_type(
            ctx,
            "createImageData expects ImageData or a size",
        ));
    };
    let Some(bytes) = crate::js::world::canvas_bytes(width, height) else {
        return Err(crate::js::bindings::throw_dom(
            ctx,
            "IndexSizeError",
            "ImageData area is too large",
        ));
    };
    image_data_object(ctx, width, height, vec![0; bytes])
}

fn image_data_object<'js>(
    ctx: &Ctx<'js>,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
) -> Result<Value<'js>> {
    let ctor: Function = ctx.globals().get("ImageData")?;
    let prototype: Object = ctor.get("prototype")?;
    let image = Object::new(ctx.clone())?;
    image.set_prototype(Some(&prototype))?;
    let data = data_array(ctx, bytes)?.into_value();
    set_image_data(
        ctx,
        &image,
        width,
        height,
        data,
        "srgb".to_owned(),
        "rgba-unorm8".to_owned(),
    )?;
    Ok(image.into_value())
}

fn read_image_data(value: &Value<'_>) -> Result<Option<(u32, u32, Vec<u8>)>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Ok(width) = object.get::<_, u32>("width") else {
        return Ok(None);
    };
    let Ok(height) = object.get::<_, u32>("height") else {
        return Ok(None);
    };
    let Some(data) = object.get::<_, Value>("data").ok() else {
        return Ok(None);
    };
    let Some(bytes) = typed_array_bytes(&data)? else {
        return Ok(None);
    };
    Ok(Some((width, height, bytes)))
}

fn typed_array_bytes(value: &Value<'_>) -> Result<Option<Vec<u8>>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if let Some(array) = object.as_typed_array::<rquickjs::U8Clamped>() {
        let mut bytes = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let key = u32::try_from(index).unwrap_or(u32::MAX);
            bytes.push(array.get::<_, u8>(key)?);
        }
        return Ok(Some(bytes));
    }
    if let Some(array) = object.as_typed_array::<u8>() {
        let mut bytes = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let key = u32::try_from(index).unwrap_or(u32::MAX);
            bytes.push(array.get::<_, u8>(key)?);
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn canvas_u32(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is finite, non-negative, and clamped to the u32 range"
    )]
    let value = value.min(f64::from(u32::MAX)) as u32;
    value
}

// ---- images --------------------------------------------------------------

fn draw_image<'js>(
    this: This<Object<'js>>,
    source: Value<'js>,
    args: rquickjs::function::Rest<Coerced<f64>>,
) -> Result<()> {
    let Some(image) = source_image(this.0.ctx(), source)? else {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "InvalidStateError",
            "image is not decoded",
        ));
    };
    let values: Vec<f64> = args.0.iter().map(|value| **value).collect();
    let (sx, sy, sw, sh, dx, dy, dw, dh) = match values.as_slice() {
        [dx, dy] => (
            0.0,
            0.0,
            f64::from(image.width),
            f64::from(image.height),
            *dx,
            *dy,
            f64::from(image.width),
            f64::from(image.height),
        ),
        [dx, dy, dw, dh] => (
            0.0,
            0.0,
            f64::from(image.width),
            f64::from(image.height),
            *dx,
            *dy,
            *dw,
            *dh,
        ),
        [sx, sy, sw, sh, dx, dy, dw, dh] => (*sx, *sy, *sw, *sh, *dx, *dy, *dw, *dh),
        _ => {
            return Err(rquickjs::Exception::throw_type(
                this.0.ctx(),
                "drawImage expects 3, 5, or 9 arguments",
            ));
        }
    };
    with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.draw_image(
            &image,
            f32_of(sx),
            f32_of(sy),
            f32_of(sw),
            f32_of(sh),
            f32_of(dx),
            f32_of(dy),
            f32_of(dw),
            f32_of(dh),
        );
    })
}

/// Decodes a `drawImage`/`createPattern` source: a canvas element, an
/// `ImageData`, or an `<img>` with decoded pixels.
fn source_image<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Option<SourceImage>> {
    if let Some(id) = crate::js::bindings::host_node_id(ctx, &value) {
        let world = world(ctx)?;
        let registry = world.borrow().registry();
        if let Some(image) = registry.borrow().canvas_snapshot(id) {
            return Ok(Some(image));
        }
        if let Some(image) = world
            .borrow()
            .with_document(id, |parsed| {
                let element = parsed.document.base.get_node(id.node)?;
                let data = element.data.downcast_element()?;
                if data.name.ns == crate::dom::tree::html_namespace()
                    && data.name.local.as_str() == "canvas"
                {
                    // A canvas without a context is still an empty bitmap.
                    let width = canvas_dimension(ctx, id, "width", 300);
                    let height = canvas_dimension(ctx, id, "height", 150);
                    return Some(SourceImage {
                        width,
                        height,
                        pixels: vec![0; width as usize * height as usize * 4],
                    });
                }
                let raster = data.raster_image_data()?;
                Some(SourceImage {
                    width: raster.width,
                    height: raster.height,
                    pixels: raster.data.as_ref().to_vec(),
                })
            })
            .flatten()
        {
            return Ok(Some(image));
        }
    }
    let Some((width, height, pixels)) = read_image_data(&value)? else {
        return Ok(None);
    };
    Ok(Some(SourceImage {
        width,
        height,
        pixels,
    }))
}

// ---- text ----------------------------------------------------------------

#[expect(clippy::unnecessary_wraps, reason = "rquickjs callbacks return Result")]
fn fill_text(
    this: This<Object<'_>>,
    _text: DomString,
    _x: Coerced<f64>,
    _y: Coerced<f64>,
    _max_width: Opt<Coerced<f64>>,
) -> Result<()> {
    let _ = this;
    Ok(())
}

#[expect(clippy::unnecessary_wraps, reason = "rquickjs callbacks return Result")]
fn stroke_text(
    this: This<Object<'_>>,
    _text: DomString,
    _x: Coerced<f64>,
    _y: Coerced<f64>,
    _max_width: Opt<Coerced<f64>>,
) -> Result<()> {
    let _ = this;
    Ok(())
}

fn measure_text(this: This<Object<'_>>, _text: DomString) -> Result<Object<'_>> {
    let object = Object::new(this.0.ctx().clone())?;
    object.set("width", 0.0)?;
    object.set("actualBoundingBoxLeft", 0.0)?;
    object.set("actualBoundingBoxRight", 0.0)?;
    object.set("actualBoundingBoxAscent", 0.0)?;
    object.set("actualBoundingBoxDescent", 0.0)?;
    object.set("fontBoundingBoxAscent", 0.0)?;
    object.set("fontBoundingBoxDescent", 0.0)?;
    object.set("emHeightAscent", 0.0)?;
    object.set("emHeightDescent", 0.0)?;
    object.set("hangingBaseline", 0.0)?;
    object.set("alphabeticBaseline", 0.0)?;
    object.set("ideographicBaseline", 0.0)?;
    Ok(object)
}

// ---- style properties ----------------------------------------------------

fn get_fill_style(this: This<Object<'_>>) -> Result<Value<'_>> {
    style_value(this.0.ctx(), &this.0, state::Canvas2D::fill_style)
}

fn set_fill_style<'js>(this: This<Object<'js>>, value: Value<'js>) -> Result<()> {
    set_style_value(
        this.0.ctx(),
        &this.0,
        value,
        state::Canvas2D::set_fill_style,
    )
}

fn get_stroke_style(this: This<Object<'_>>) -> Result<Value<'_>> {
    style_value(this.0.ctx(), &this.0, state::Canvas2D::stroke_style)
}

fn set_stroke_style<'js>(this: This<Object<'js>>, value: Value<'js>) -> Result<()> {
    set_style_value(
        this.0.ctx(),
        &this.0,
        value,
        state::Canvas2D::set_stroke_style,
    )
}

fn style_value<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    read: impl FnOnce(&Canvas2D) -> StyleValue,
) -> Result<Value<'js>> {
    let style = with_canvas(ctx, object, |canvas| read(canvas))?;
    match style {
        StyleValue::Color(color) => {
            Ok(rquickjs::String::from_str(ctx.clone(), &serialize_color(color))?.into_value())
        }
        StyleValue::Gradient(id) | StyleValue::Pattern(id) => {
            let resources: Object = object.get("__tbRes")?;
            if let Ok(value) = resources.get::<_, Value>(id.to_string()) {
                Ok(value)
            } else {
                Ok(Value::new_undefined(ctx.clone()))
            }
        }
    }
}

fn set_style_value<'js>(
    ctx: &Ctx<'js>,
    object: &Object<'js>,
    value: Value<'js>,
    set: impl FnOnce(&mut Canvas2D, StyleValue),
) -> Result<()> {
    let style = if let Some(string) = value.as_string() {
        let text = string.to_string()?;
        match css_color(ctx, object, &text)? {
            Some(color) => Some(StyleValue::Color(color)),
            None => return Ok(()),
        }
    } else if let Some(gradient) = value
        .as_object()
        .and_then(|object| resource_id(object, "Gradient"))
    {
        Some(StyleValue::Gradient(gradient))
    } else if let Some(pattern) = value
        .as_object()
        .and_then(|object| resource_id(object, "Pattern"))
    {
        Some(StyleValue::Pattern(pattern))
    } else if value.is_object() {
        return Ok(());
    } else {
        let text = Coerced::<rquickjs::String<'_>>::from_js(ctx, value)?.to_string()?;
        match css_color(ctx, object, &text)? {
            Some(color) => Some(StyleValue::Color(color)),
            None => return Ok(()),
        }
    };
    let Some(style) = style else {
        return Ok(());
    };
    with_canvas(ctx, object, |canvas| set(canvas, style))
}

/// Parses a canvas color. `currentColor` resolves against the canvas
/// element's computed `color` at assignment time, not at paint time
/// (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fillstyle>).
fn css_color(ctx: &Ctx<'_>, object: &Object<'_>, text: &str) -> Result<Option<[u8; 4]>> {
    if let Some(color) = color::parse(text) {
        return Ok(Some(color));
    }
    if text.trim().eq_ignore_ascii_case("currentColor") {
        return Ok(Some(current_color(ctx, object)?));
    }
    Ok(None)
}

/// The canvas element's computed `color`, or black when it has no computed
/// style (detached elements have none)
/// (<https://drafts.csswg.org/css-color-4/#currentcolor-color>).
fn current_color(ctx: &Ctx<'_>, object: &Object<'_>) -> Result<[u8; 4]> {
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    let id = canvas_id(object)?;
    let world = world(ctx)?;
    let world = world.borrow();
    // `__tbDoc`/`__tbNode` are writable; do not read another frame's
    // stylesheet for a forged context.
    if !world.owns_node(id) {
        return Ok(BLACK);
    }
    let Some(mut parsed) = world.document_mut(id) else {
        return Ok(BLACK);
    };
    let base = &mut parsed.document.base;
    base.resolve(0.0);
    let text = base.resolved_style_value(id.node, "color");
    Ok(color::parse(&text).unwrap_or(BLACK))
}

#[cfg(test)]
mod tests {
    use super::color;

    #[test]
    fn parses_basic_colors() {
        assert_eq!(color::parse("red"), Some([255, 0, 0, 255]));
        assert_eq!(color::parse("#00ff0080"), Some([0, 255, 0, 128]));
        assert_eq!(color::parse("rgb(0 0 0 / 50%)"), Some([0, 0, 0, 128]));
    }
}
