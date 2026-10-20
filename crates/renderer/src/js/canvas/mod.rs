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
mod path2d;
mod state;
mod text;

pub(crate) use state::{Canvas2D, SourceImage, StyleValue};

use rquickjs::function::This;
use rquickjs::object::Accessor;
use rquickjs::prelude::{Func, Opt};
use rquickjs::{
    Class, Coerced, Ctx, Exception, FromJs, Function, Object, Result, TypedArray, Value,
};

use crate::dom::tree::NodeId;
use crate::js::bindings::{world, wrap_node};
use path2d::JsPath2D;
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
    install_gradient_objects(ctx)?;
    install_path2d(ctx)?;
    install_image_data(ctx)?;
    install_image_bitmap(ctx)?;
    Ok(())
}

fn install_path2d(ctx: &Ctx<'_>) -> Result<()> {
    path2d::install(ctx)
}

/// `CanvasGradient` and `CanvasPattern` constructors: canvas returns branded
/// instances, and the globals make `instanceof` and prototype checks work
/// (<https://html.spec.whatwg.org/multipage/canvas.html#canvasgradient>).
fn install_gradient_objects(ctx: &Ctx<'_>) -> Result<()> {
    for name in ["CanvasGradient", "CanvasPattern"] {
        let prototype = Object::new(ctx.clone())?;
        if name == "CanvasGradient" {
            prototype.set("addColorStop", Func::from(add_color_stop))?;
        }
        let ctor = Function::new(ctx.clone(), || {})?;
        ctor.set("prototype", prototype.clone())?;
        prototype.set("constructor", ctor.clone())?;
        ctx.globals().set(name, ctor)?;
    }
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
    method!("beginLayer", 0, begin_layer);
    method!("endLayer", 0, end_layer);
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
    method!("createConicGradient", 3, create_conic_gradient);
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
    prototype.prop("canvas", Accessor::from(canvas_object))?;
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
    prototype.prop("filter", Accessor::new(get_filter, set_filter_binding))?;
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
        "textRendering",
        text_rendering,
        set_text_rendering
    );
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

/// `WebIDL` `[EnforceRange] long`: reject non-finite values and values outside
/// the long range with `TypeError`, then truncate.
fn enforce_long(ctx: &Ctx<'_>, value: f64) -> Result<i32> {
    if !value.is_finite() {
        return Err(Exception::throw_type(ctx, "value must be finite"));
    }
    let value = value.trunc();
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(Exception::throw_type(ctx, "value is outside the long range"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is finite, integral, and inside the i32 range"
    )]
    Ok(value as i32)
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
    let restored = with_canvas(this.0.ctx(), &this.0, state::Canvas2D::restore)?;
    if restored {
        Ok(())
    } else {
        Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "InvalidStateError",
            "restore() cannot cross a layer boundary",
        ))
    }
}

fn begin_layer<'js>(this: This<Object<'js>>, options: Opt<Value<'js>>) -> Result<()> {
    let ctx = this.0.ctx();
    if let Some(value) = options.0 {
        if value.is_null() || value.is_undefined() {
            return with_canvas(ctx, &this.0, state::Canvas2D::begin_layer);
        }
        if !value.is_object() {
            return Err(Exception::throw_type(
                ctx,
                "beginLayer options must be an object",
            ));
        }
        if let Some(object) = value.as_object()
            && let Ok(filter) = object.get::<_, Value>("filter")
            && filter.is_object()
            && !filter.is_null()
            && let Ok(validate) = ctx.globals().get::<_, Function>("__tbValidateFilter")
        {
            validate.call::<_, ()>((filter,))?;
        }
    }
    with_canvas(ctx, &this.0, state::Canvas2D::begin_layer)
}

fn end_layer(this: This<Object<'_>>) -> Result<()> {
    let closed = with_canvas(this.0.ctx(), &this.0, state::Canvas2D::end_layer)?;
    if closed {
        Ok(())
    } else {
        Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "InvalidStateError",
            "endLayer() without a matching beginLayer()",
        ))
    }
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

/// `CanvasFillRule`: an absent rule is nonzero, and any other string is a
/// `TypeError`
/// (<https://html.spec.whatwg.org/multipage/canvas.html#canvasfillrule>).
fn fill_rule(ctx: &Ctx<'_>, rule: Option<&DomString>) -> Result<bool> {
    match rule {
        None => Ok(false),
        Some(rule) => match &**rule {
            "nonzero" | "undefined" => Ok(false),
            "evenodd" => Ok(true),
            _ => Err(Exception::throw_type(ctx, "invalid fill rule")),
        },
    }
}

fn fill<'js>(
    this: This<Object<'js>>,
    first: Opt<Value<'js>>,
    second: Opt<DomString>,
) -> Result<()> {
    let ctx = this.0.ctx();
    if let Some(value) = first.0 {
        if value.is_undefined() {
            if second.0.is_some() {
                return Err(Exception::throw_type(ctx, "fill expects a Path2D"));
            }
            return with_canvas(ctx, &this.0, |canvas| canvas.fill(false));
        }
        if let Ok(path) = Class::<JsPath2D>::from_value(&value) {
            let even_odd = fill_rule(ctx, second.0.as_ref())?;
            let path = path.borrow();
            return with_canvas(ctx, &this.0, |canvas| {
                canvas.fill_borrowed_path(&path.canvas.borrow(), even_odd);
            });
        }
        // The two-argument overload only accepts a Path2D first.
        if second.0.is_some() {
            return Err(Exception::throw_type(ctx, "fill expects a Path2D"));
        }
        let rule = DomString::from_js(ctx, value)?;
        let even_odd = fill_rule(ctx, Some(&rule))?;
        return with_canvas(ctx, &this.0, |canvas| canvas.fill(even_odd));
    }
    with_canvas(ctx, &this.0, |canvas| canvas.fill(false))
}

fn stroke<'js>(this: This<Object<'js>>, path: Opt<Value<'js>>) -> Result<()> {
    let ctx = this.0.ctx();
    if let Some(value) = path.0 {
        if value.is_undefined() {
            return with_canvas(ctx, &this.0, state::Canvas2D::stroke);
        }
        let Ok(path) = Class::<JsPath2D>::from_value(&value) else {
            return Err(Exception::throw_type(ctx, "stroke expects a Path2D"));
        };
        let path = path.borrow();
        return with_canvas(ctx, &this.0, |canvas| {
            canvas.stroke_borrowed_path(&path.canvas.borrow());
        });
    }
    with_canvas(ctx, &this.0, state::Canvas2D::stroke)
}

fn clip<'js>(
    this: This<Object<'js>>,
    first: Opt<Value<'js>>,
    second: Opt<DomString>,
) -> Result<()> {
    let ctx = this.0.ctx();
    if let Some(value) = first.0 {
        if value.is_undefined() {
            if second.0.is_some() {
                return Err(Exception::throw_type(ctx, "clip expects a Path2D"));
            }
            return with_canvas(ctx, &this.0, |canvas| canvas.clip(false));
        }
        if let Ok(path) = Class::<JsPath2D>::from_value(&value) {
            let even_odd = fill_rule(ctx, second.0.as_ref())?;
            let path = path.borrow();
            return with_canvas(ctx, &this.0, |canvas| {
                canvas.clip_borrowed_path(&path.canvas.borrow(), even_odd);
            });
        }
        if second.0.is_some() {
            return Err(Exception::throw_type(ctx, "clip expects a Path2D"));
        }
        let rule = DomString::from_js(ctx, value)?;
        let even_odd = fill_rule(ctx, Some(&rule))?;
        return with_canvas(ctx, &this.0, |canvas| canvas.clip(even_odd));
    }
    with_canvas(ctx, &this.0, |canvas| canvas.clip(false))
}

fn is_point_in_path<'js>(
    this: This<Object<'js>>,
    args: rquickjs::function::Rest<Value<'js>>,
) -> Result<bool> {
    let ctx = this.0.ctx();
    let (path, offset) = match args.0.first() {
        Some(value) => match Class::<JsPath2D>::from_value(value) {
            Ok(path) => (Some(path), 1),
            Err(_) => (None, 0),
        },
        None => (None, 0),
    };
    let x = args
        .0
        .get(offset)
        .ok_or_else(|| Exception::throw_type(ctx, "isPointInPath needs x"))?;
    let y = args
        .0
        .get(offset + 1)
        .ok_or_else(|| Exception::throw_type(ctx, "isPointInPath needs y"))?;
    let x = f32_of(*Coerced::<f64>::from_js(ctx, x.clone())?);
    let y = f32_of(*Coerced::<f64>::from_js(ctx, y.clone())?);
    let rule = args
        .0
        .get(offset + 2)
        .map(|value| DomString::from_js(ctx, value.clone()))
        .transpose()?;
    let even_odd = fill_rule(ctx, rule.as_ref())?;
    let Some(path) = path else {
        return with_canvas(ctx, &this.0, |canvas| {
            canvas.is_point_in_path(x, y, even_odd)
        });
    };
    let path = path.borrow();
    with_canvas(ctx, &this.0, |canvas| {
        canvas.is_point_in_borrowed_path(&path.canvas.borrow(), x, y, even_odd)
    })
}

fn is_point_in_stroke<'js>(
    this: This<Object<'js>>,
    args: rquickjs::function::Rest<Value<'js>>,
) -> Result<bool> {
    let ctx = this.0.ctx();
    let (path, offset) = match args.0.first() {
        Some(value) => match Class::<JsPath2D>::from_value(value) {
            Ok(path) => (Some(path), 1),
            Err(_) => (None, 0),
        },
        None => (None, 0),
    };
    let x = args
        .0
        .get(offset)
        .ok_or_else(|| Exception::throw_type(ctx, "isPointInStroke needs x"))?;
    let y = args
        .0
        .get(offset + 1)
        .ok_or_else(|| Exception::throw_type(ctx, "isPointInStroke needs y"))?;
    let x = f32_of(*Coerced::<f64>::from_js(ctx, x.clone())?);
    let y = f32_of(*Coerced::<f64>::from_js(ctx, y.clone())?);
    let Some(path) = path else {
        return with_canvas(ctx, &this.0, |canvas| canvas.is_point_in_stroke(x, y));
    };
    let path = path.borrow();
    with_canvas(ctx, &this.0, |canvas| {
        canvas.is_point_in_borrowed_stroke(&path.canvas.borrow(), x, y)
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
    if ![*x0, *y0, *x1, *y1].iter().all(|value| value.is_finite()) {
        return Err(Exception::throw_type(
            this.0.ctx(),
            "gradient coordinates must be finite",
        ));
    }
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
    if ![*x0, *y0, *r0, *x1, *y1, *r1]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(Exception::throw_type(
            this.0.ctx(),
            "gradient coordinates must be finite",
        ));
    }
    if *r0 < 0.0 || *r1 < 0.0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "radii must not be negative",
        ));
    }
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
    if ![*x, *y, *angle].iter().all(|value| value.is_finite()) {
        return Err(Exception::throw_type(
            this.0.ctx(),
            "gradient coordinates must be finite",
        ));
    }
    let id = with_canvas(this.0.ctx(), &this.0, |canvas| {
        canvas.create_conic_gradient(f32_of(*x), f32_of(*y), f32_of(*angle))
    })?;
    resource_object(&this.0, "Gradient", id)
}

fn create_pattern<'js>(
    this: This<Object<'js>>,
    source: Value<'js>,
    repetition: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let repetition = match repetition.0 {
        None => "repeat".to_owned(),
        Some(value) if value.is_null() => "repeat".to_owned(),
        Some(value) => {
            let ctx = value.ctx().clone();
            DomString::from_js(&ctx, value)?.to_string()
        }
    };
    if !matches!(
        repetition.as_str(),
        "repeat" | "repeat-x" | "repeat-y" | "no-repeat" | ""
    ) {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "SyntaxError",
            "invalid repetition value",
        ));
    }
    let repetition = if repetition.is_empty() {
        "repeat".to_owned()
    } else {
        repetition
    };
    let source = match source_image(this.0.ctx(), source)? {
        ImageLookup::Decoded(source) => source,
        ImageLookup::NotReady => return Ok(Value::new_null(this.0.ctx().clone())),
        ImageLookup::Unsupported => {
            return Err(Exception::throw_type(
                this.0.ctx(),
                "createPattern expects an image source",
            ));
        }
        ImageLookup::Invalid => {
            return Err(crate::js::bindings::throw_dom(
                this.0.ctx(),
                "InvalidStateError",
                "the image source is not valid",
            ));
        }
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
    let ctor_name = if kind == "Gradient" {
        "CanvasGradient"
    } else {
        "CanvasPattern"
    };
    // Use the intrinsic prototype captured at install time. The global is
    // page-writable, the intrinsic is not.
    let prototype: Object = if let Some(prototype) =
        world(this.ctx())?.borrow().brand(ctor_name)
    {
        prototype.restore(this.ctx())?
    } else {
        let ctor: Function = this.ctx().globals().get(ctor_name)?;
        ctor.get("prototype")?
    };
    object.set_prototype(Some(&prototype))?;
    object.set("__tbDoc", this.get::<_, u32>("__tbDoc")?)?;
    object.set("__tbNode", this.get::<_, u64>("__tbNode")?)?;
    object.set(format!("__tb{kind}"), id)?;
    let resources: Object = this.get("__tbRes")?;
    resources.set(id.to_string(), object.clone())?;
    Ok(object.into_value())
}

fn add_color_stop(this: This<Object<'_>>, offset: Coerced<f64>, color: DomString) -> Result<()> {
    let id: u32 = this.0.get("__tbGradient")?;
    if !offset.is_finite() {
        return Err(Exception::throw_type(
            this.0.ctx(),
            "color stop offset must be finite",
        ));
    }
    // The offset check precedes color parsing, so an out-of-range offset
    // reports IndexSizeError even when the color is also invalid.
    if !(0.0..=1.0).contains(&*offset) {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "offset out of range",
        ));
    }
    // `currentColor` in a gradient stop resolves to opaque black: canvas
    // color stops do not inherit the element's CSS color
    // (<https://html.spec.whatwg.org/multipage/canvas.html#dom-canvasgradient-addcolorstop>).
    let color = if color.trim().eq_ignore_ascii_case("currentColor") {
        [0, 0, 0, 255]
    } else if let Some(color) = css_color(this.0.ctx(), &this.0, &color)? {
        color
    } else {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "SyntaxError",
            "invalid color for addColorStop",
        ));
    };
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
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|bytes| bytes.checked_mul(4));
    if height == 0 || expected != Some(byte_length as u64) {
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
    // The IDL marks every argument `[EnforceRange] long`.
    let (mut sx, mut sy, mut sw, mut sh) = (
        f64::from(enforce_long(this.0.ctx(), *sx)?),
        f64::from(enforce_long(this.0.ctx(), *sy)?),
        f64::from(enforce_long(this.0.ctx(), *sw)?),
        f64::from(enforce_long(this.0.ctx(), *sh)?),
    );
    if sw == 0.0 || sh == 0.0 {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "IndexSizeError",
            "source width or height is zero",
        ));
    }
    // Negative extents move the origin and normalize, like Blink
    // (`base_rendering_context_2d.cc`, getImageDataInternal).
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
    let (pixels, origin_clean) = with_canvas(this.0.ctx(), &this.0, |canvas| {
        (canvas.read_pixels(x, y, width, height), canvas.origin_clean())
    })?;
    if !origin_clean {
        return Err(crate::js::bindings::throw_dom(
            this.0.ctx(),
            "SecurityError",
            "the canvas is not origin-clean",
        ));
    }
    let bytes = pixels.ok_or_else(|| {
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
    let (dx, dy) = (
        enforce_long(ctx, number(0)?)?,
        enforce_long(ctx, number(1)?)?,
    );
    let Some((width, height, pixels)) = read_image_data(&data)? else {
        return Err(Exception::throw_type(ctx, "putImageData expects ImageData"));
    };
    let dirty = match (number(2), number(3), number(4), number(5)) {
        (Ok(x), Ok(y), Ok(width), Ok(height)) => Some((
            enforce_long(ctx, x)?,
            enforce_long(ctx, y)?,
            enforce_long(ctx, width)?,
            enforce_long(ctx, height)?,
        )),
        _ => None,
    };
    with_canvas(ctx, &this.0, |canvas| {
        canvas.write_pixels(&pixels, width, height, dx, dy, dirty);
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
        // Blink takes the absolute value of both dimensions
        // (`BaseRenderingContext2D::createImageData`), but both are
        // `[EnforceRange] long` first.
        let width = enforce_long(ctx, width)?.unsigned_abs();
        let height = enforce_long(ctx, height)?.unsigned_abs();
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
    let usable = |value: Value<'_>| !value.is_undefined() && !value.is_null();
    let data = object
        .get::<_, Value>("data")
        .ok()
        .filter(|value| usable(value.clone()))
        .or_else(|| {
            object
                .get::<_, Value>("__tbData")
                .ok()
                .filter(|value| usable(value.clone()))
        });
    let Some(data) = data else {
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

// ---- image bitmaps -------------------------------------------------------

/// `ImageBitmap` and the host half of `createImageBitmap`
/// (<https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#imagebitmaps>).
fn install_image_bitmap(ctx: &Ctx<'_>) -> Result<()> {
    let prototype = Object::new(ctx.clone())?;
    prototype.prop(
        "width",
        Accessor::new(image_bitmap_width, image_data_read_only),
    )?;
    prototype.prop(
        "height",
        Accessor::new(image_bitmap_height, image_data_read_only),
    )?;
    prototype.prop("close", Func::from(image_bitmap_close))?;
    let ctor = Function::new(ctx.clone(), || {})?;
    ctor.set("prototype", prototype)?;
    ctx.globals().set("ImageBitmap", ctor)?;
    let host = crate::js::bridge::object(ctx)?;
    host.set(
        "createImageBitmapDecode",
        Func::from(create_image_bitmap_decode),
    )?;
    host.set(
        "createImageBitmapFromSource",
        Func::from(create_image_bitmap_from_source),
    )?;
    Ok(())
}

fn image_bitmap_width(this: This<Object<'_>>) -> Result<u32> {
    this.0.get("__tbWidth")
}

fn image_bitmap_height(this: This<Object<'_>>) -> Result<u32> {
    this.0.get("__tbHeight")
}

fn image_bitmap_close(this: This<Object<'_>>) -> Result<()> {
    if let Ok(id) = this.0.get::<_, u64>("__tbBitmap")
        && let Ok(world) = world(this.0.ctx())
    {
        world.borrow().registry().borrow_mut().remove_bitmap(id);
    }
    this.0.set("__tbWidth", 0)?;
    this.0.set("__tbHeight", 0)?;
    this.0.set("__tbClosed", true)?;
    Ok(())
}

fn create_image_bitmap_decode<'js>(
    ctx: Ctx<'js>,
    bytes: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let Some(bytes) = typed_array_bytes(&bytes)? else {
        return Err(Exception::throw_type(
            &ctx,
            "createImageBitmap expects a BufferSource",
        ));
    };
    let options = bitmap_options(options.0.as_ref().and_then(|value| value.as_object()))?;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| {
            crate::js::bindings::throw_dom(&ctx, "InvalidStateError", "image could not be read")
        })?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(crate::js::world::MAX_CANVAS_BYTES as u64);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| {
        crate::js::bindings::throw_dom(&ctx, "InvalidStateError", "image could not be decoded")
    })?;
    if crate::js::world::canvas_bytes(image.width(), image.height()).is_none() {
        return Err(crate::js::bindings::throw_dom(
            &ctx,
            "InvalidStateError",
            "image is too large",
        ));
    }
    let image = image.to_rgba8();
    let (width, height) = (image.width(), image.height());
    let Some((width, height, pixels)) = resize_bitmap(width, height, image.into_raw(), &options)
    else {
        return Err(crate::js::bindings::throw_dom(
            &ctx,
            "InvalidStateError",
            "bitmap is too large",
        ));
    };
    image_bitmap_object(&ctx, width, height, pixels, true)
}

fn create_image_bitmap_from_source<'js>(
    ctx: Ctx<'js>,
    source: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let options = bitmap_options(options.0.as_ref().and_then(|value| value.as_object()))?;
    let image = match source_image(&ctx, source)? {
        ImageLookup::Decoded(image) => image,
        ImageLookup::Unsupported => {
            return Err(Exception::throw_type(
                &ctx,
                "createImageBitmap cannot use this source",
            ));
        }
        ImageLookup::NotReady | ImageLookup::Invalid => {
            return Err(crate::js::bindings::throw_dom(
                &ctx,
                "InvalidStateError",
                "the image source is not usable",
            ));
        }
    };
    let origin_clean = image.origin_clean;
    let Some((width, height, pixels)) =
        resize_bitmap(image.width, image.height, image.pixels, &options)
    else {
        return Err(crate::js::bindings::throw_dom(
            &ctx,
            "InvalidStateError",
            "bitmap is too large",
        ));
    };
    image_bitmap_object(&ctx, width, height, pixels, origin_clean)
}

fn image_bitmap_object<'js>(
    ctx: &Ctx<'js>,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    origin_clean: bool,
) -> Result<Value<'js>> {
    let ctor: Function = ctx.globals().get("ImageBitmap")?;
    let prototype: Object = ctor.get("prototype")?;
    let image = Object::new(ctx.clone())?;
    image.set_prototype(Some(&prototype))?;
    let bitmap_id = world(ctx)?
        .borrow()
        .registry()
        .borrow_mut()
        .insert_bitmap(width, height, pixels, origin_clean);
    image.set("__tbBitmap", bitmap_id)?;
    image.set("__tbWidth", width)?;
    image.set("__tbHeight", height)?;
    image.set(
        "__tbColorSpace",
        rquickjs::String::from_str(ctx.clone(), "srgb")?,
    )?;
    image.set(
        "__tbPixelFormat",
        rquickjs::String::from_str(ctx.clone(), "rgba-unorm8")?,
    )?;
    image.set("__tbOriginClean", origin_clean)?;
    image.set("__tbClosed", false)?;
    Ok(image.into_value())
}

struct BitmapOptions {
    resize_width: Option<u32>,
    resize_height: Option<u32>,
    quality: image::imageops::FilterType,
    flip_y: bool,
}

fn bitmap_options(options: Option<&Object<'_>>) -> Result<BitmapOptions> {
    let read = |name: &str| {
        options
            .and_then(|object| object.get::<_, Value>(name).ok())
            .filter(|value| !value.is_undefined() && !value.is_null())
    };
    let dimension = |value: Option<Value<'_>>| -> Result<Option<u32>> {
        let Some(value) = value else {
            return Ok(None);
        };
        let ctx = value.ctx().clone();
        let number = *Coerced::<f64>::from_js(&ctx, value)?;
        if !number.is_finite() {
            return Err(Exception::throw_type(
                &ctx,
                "resize dimensions must be finite",
            ));
        }
        let number = canvas_u32(number);
        if number == 0 {
            return Err(Exception::throw_type(
                &ctx,
                "resize dimensions must be nonzero",
            ));
        }
        Ok(Some(number))
    };
    let resize_width = dimension(read("resizeWidth"))?;
    let resize_height = dimension(read("resizeHeight"))?;
    let quality = read("resizeQuality")
        .map(|value| {
            let ctx = value.ctx().clone();
            DomString::from_js(&ctx, value)
        })
        .transpose()?
        .map_or(
            image::imageops::FilterType::Triangle,
            |value| match &*value {
                "pixelated" => image::imageops::FilterType::Nearest,
                "medium" => image::imageops::FilterType::CatmullRom,
                "high" => image::imageops::FilterType::Lanczos3,
                _ => image::imageops::FilterType::Triangle,
            },
        );
    let flip_y = read("imageOrientation")
        .map(|value| {
            let ctx = value.ctx().clone();
            DomString::from_js(&ctx, value)
        })
        .transpose()?
        .is_some_and(|value| &*value == "flipY");
    Ok(BitmapOptions {
        resize_width,
        resize_height,
        quality,
        flip_y,
    })
}

fn resize_bitmap(
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    options: &BitmapOptions,
) -> Option<(u32, u32, Vec<u8>)> {
    let buffer = image::RgbaImage::from_raw(width, height, pixels)?;
    let (source_width, source_height) = (buffer.width(), buffer.height());
    let (target_width, target_height) = match (options.resize_width, options.resize_height) {
        (None, None) => (source_width, source_height),
        (Some(width), None) => (width, scaled_side(source_height, width, source_width)),
        (None, Some(height)) => (scaled_side(source_width, height, source_height), height),
        (Some(width), Some(height)) => (width, height),
    };
    crate::js::world::canvas_bytes(target_width, target_height)?;
    let resized = if (target_width, target_height) == (source_width, source_height) {
        buffer
    } else {
        image::imageops::resize(&buffer, target_width, target_height, options.quality)
    };
    let resized = if options.flip_y {
        image::imageops::flip_vertical(&resized)
    } else {
        resized
    };
    Some((resized.width(), resized.height(), resized.into_raw()))
}

/// The unspecified resize axis keeps the source aspect ratio, rounded to the
/// nearest pixel
/// (<https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#dom-createimagebitmap>).
fn scaled_side(side: u32, target: u32, source: u32) -> u32 {
    if source == 0 {
        return 1;
    }
    let ratio = f64::from(target) / f64::from(source);
    let scaled = (f64::from(side) * ratio).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the scale is positive and bounded by the target dimension"
    )]
    let scaled = scaled.max(1.0) as u32;
    scaled
}

// ---- images --------------------------------------------------------------

fn draw_image<'js>(
    this: This<Object<'js>>,
    source: Value<'js>,
    args: rquickjs::function::Rest<Coerced<f64>>,
) -> Result<()> {
    let image = match source_image(this.0.ctx(), source)? {
        ImageLookup::Decoded(image) => image,
        ImageLookup::NotReady => return Ok(()),
        ImageLookup::Unsupported => {
            return Err(Exception::throw_type(
                this.0.ctx(),
                "drawImage expects an image source",
            ));
        }
        ImageLookup::Invalid => {
            return Err(crate::js::bindings::throw_dom(
                this.0.ctx(),
                "InvalidStateError",
                "the image source is not usable",
            ));
        }
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

/// A `drawImage`/`createPattern` source lookup.
enum ImageLookup {
    Decoded(SourceImage),
    /// A canvas or image that exists but has no pixels yet; drawing it is a
    /// no-op, not an error.
    NotReady,
    /// The object is not an image source at all.
    Unsupported,
    /// The source is invalid (zero-sized canvas, closed bitmap).
    Invalid,
}

/// Decodes a `drawImage`/`createPattern` source: a canvas element, an
/// `ImageData`/`ImageBitmap`, or an `<img>` with decoded pixels
/// (<https://html.spec.whatwg.org/multipage/canvas.html#check-the-usability-of-the-image-argument>).
fn source_image<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<ImageLookup> {
    if let Some(id) = crate::js::bindings::host_node_id(ctx, &value) {
        let world = world(ctx)?;
        let (loading, broken) = {
            let world = world.borrow();
            (
                world.images.loading.contains(&id),
                world.images.broken.contains(&id),
            )
        };
        if loading {
            return Ok(ImageLookup::NotReady);
        }
        if broken {
            return Ok(ImageLookup::Invalid);
        }
        if let Some(snapshot) = world.borrow().registry().borrow().canvas_snapshot(id) {
            return Ok(if snapshot.width == 0 || snapshot.height == 0 {
                ImageLookup::Invalid
            } else {
                ImageLookup::Decoded(snapshot)
            });
        }
        let mut lookup = world
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
                    if width == 0 || height == 0 {
                        return Some(ImageLookup::Invalid);
                    }
                    let pixels = crate::js::world::canvas_bytes(width, height)
                        .map_or_else(Vec::new, |bytes| vec![0; bytes]);
                    return Some(ImageLookup::Decoded(SourceImage {
                        width,
                        height,
                        pixels,
                        origin_clean: true,
                    }));
                }
                if !(data.name.ns == crate::dom::tree::html_namespace()
                    && data.name.local.as_str() == "img")
                {
                    return Some(ImageLookup::Unsupported);
                }
                match data.raster_image_data() {
                    Some(raster) => Some(ImageLookup::Decoded(SourceImage {
                        width: raster.width,
                        height: raster.height,
                        pixels: raster.data.as_ref().to_vec(),
                        origin_clean: true,
                    })),
                    // Still loading, or failed to load: drawing paints
                    // nothing and does not throw.
                    None => Some(ImageLookup::NotReady),
                }
            })
            .flatten()
            .unwrap_or(ImageLookup::Unsupported);
        if let ImageLookup::Decoded(image) = &mut lookup {
            image.origin_clean &= image_is_same_origin(&world.borrow(), id);
        }
        return Ok(lookup);
    }
    if let Some(object) = value.as_object()
        && object.get::<_, bool>("__tbClosed").unwrap_or(false)
    {
        return Ok(ImageLookup::Invalid);
    }
    if let Some(object) = value.as_object()
        && let Ok(id) = object.get::<_, u64>("__tbBitmap")
    {
        let image = world(ctx)?
            .borrow()
            .registry()
            .borrow()
            .bitmap(id);
        return Ok(image.map_or(ImageLookup::Invalid, ImageLookup::Decoded));
    }
    let Some((width, height, pixels)) = read_image_data(&value)? else {
        return Ok(ImageLookup::Unsupported);
    };
    let origin_clean = value
        .as_object()
        .and_then(|object| object.get::<_, bool>("__tbOriginClean").ok())
        .unwrap_or(true);
    Ok(ImageLookup::Decoded(SourceImage {
        width,
        height,
        pixels,
        origin_clean,
    }))
}

/// Whether the image element's current request is same-origin with the
/// document. `data:` images are treated as clean, like browsers do; a URL
/// that does not parse is treated as cross-origin.
fn image_is_same_origin(world: &crate::js::world::World, id: NodeId) -> bool {
    let Some(src) = world.images.current_src.get(&id) else {
        return true;
    };
    let Ok(url) = url::Url::parse(src) else {
        return false;
    };
    if url.scheme() == "data" {
        return true;
    }
    world.origin == url.origin()
}

// ---- text ----------------------------------------------------------------

fn fill_text(
    this: This<Object<'_>>,
    text: DomString,
    x: Coerced<f64>,
    y: Coerced<f64>,
    max_width: Opt<Coerced<f64>>,
) -> Result<()> {
    draw_text(this, &text, *x, *y, max_width.0.as_deref().copied(), false)
}

fn stroke_text(
    this: This<Object<'_>>,
    text: DomString,
    x: Coerced<f64>,
    y: Coerced<f64>,
    max_width: Opt<Coerced<f64>>,
) -> Result<()> {
    draw_text(this, &text, *x, *y, max_width.0.as_deref().copied(), true)
}

fn draw_text(
    this: This<Object<'_>>,
    text: &str,
    x: f64,
    y: f64,
    max_width: Option<f64>,
    stroke: bool,
) -> Result<()> {
    if !x.is_finite() || !y.is_finite() {
        return Ok(());
    }
    let ctx = this.0.ctx();
    let (font, align, baseline) = with_canvas(ctx, &this.0, |canvas| {
        (
            canvas.font().to_owned(),
            canvas.text_align().to_owned(),
            canvas.text_baseline().to_owned(),
        )
    })?;
    let max_width = match max_width {
        None => None,
        Some(value) if value.is_finite() && value > 0.0 => Some(f32_of(value)),
        // A zero, negative, or non-finite maxWidth paints nothing
        // (<https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-filltext>).
        Some(_) => return Ok(()),
    };
    let mut font_ctx = document_font_context(ctx, &this.0)?;
    let shaped = text::shape(text, &font, max_width, &mut font_ctx);
    let Some(path) = shaped.path.as_ref() else {
        return Ok(());
    };
    let width = shaped.width * shaped.scale_x;
    let dx = match align.as_str() {
        "center" => -width / 2.0,
        "right" | "end" => -width,
        _ => 0.0,
    };
    // The shaped layout's origin is the top of the line, so shift it up by
    // the distance from the top to the chosen baseline
    // (<https://html.spec.whatwg.org/multipage/canvas.html#text-baselines>).
    let dy = match baseline.as_str() {
        "top" => 0.0,
        "hanging" => -shaped.ascent * 0.8,
        "middle" => -(shaped.ascent + shaped.descent) / 2.0,
        "bottom" => -(shaped.ascent + shaped.descent),
        "ideographic" => -(shaped.ascent + shaped.descent) + shaped.descent * 0.2,
        _ => -shaped.ascent,
    };
    let (x, y) = (f32_of(x) + dx, f32_of(y) + dy);
    with_canvas(ctx, &this.0, |canvas| {
        if stroke {
            canvas.stroke_text_path(path, x, y, shaped.scale_x);
        } else {
            canvas.fill_text_path(path, x, y, shaped.scale_x);
        }
    })
}

fn measure_text(this: This<Object<'_>>, text: DomString) -> Result<Object<'_>> {
    let ctx = this.0.ctx();
    let font = with_canvas(ctx, &this.0, |canvas| canvas.font().to_owned())?;
    let mut font_ctx = document_font_context(ctx, &this.0)?;
    let shaped = text::shape(&text, &font, None, &mut font_ctx);
    let object = Object::new(ctx.clone())?;
    object.set("width", shaped.width)?;
    object.set("actualBoundingBoxLeft", 0.0)?;
    object.set("actualBoundingBoxRight", shaped.width)?;
    object.set("actualBoundingBoxAscent", shaped.ascent)?;
    object.set("actualBoundingBoxDescent", shaped.descent)?;
    object.set("fontBoundingBoxAscent", shaped.ascent)?;
    object.set("fontBoundingBoxDescent", shaped.descent)?;
    object.set("emHeightAscent", shaped.ascent)?;
    object.set("emHeightDescent", shaped.descent)?;
    object.set("hangingBaseline", shaped.ascent * 0.8)?;
    object.set("alphabeticBaseline", 0.0)?;
    object.set("ideographicBaseline", -shaped.descent * 0.2)?;
    Ok(object)
}

fn get_filter(this: This<Object<'_>>) -> Result<Value<'_>> {
    if let Ok(value) = this.0.get::<_, Value>("__tbFilterObject")
        && !value.is_undefined()
        && !value.is_null()
    {
        return Ok(value);
    }
    let filter = with_canvas(this.0.ctx(), &this.0, |canvas| canvas.filter().to_owned())?;
    Ok(rquickjs::String::from_str(this.0.ctx().clone(), &filter)?.into_value())
}

fn set_filter_binding<'js>(this: This<Object<'js>>, value: Value<'js>) -> Result<()> {
    let ctx = this.0.ctx();
    if value.is_object() && !value.is_null() {
        this.0.set("__tbFilterObject", value)?;
        return Ok(());
    }
    let text = DomString::from_js(ctx, value)?.to_string();
    if !valid_filter(&text) {
        return Ok(());
    }
    this.0
        .set("__tbFilterObject", Value::new_undefined(ctx.clone()))?;
    with_canvas(ctx, &this.0, |canvas| canvas.set_filter(&text))
}

/// A CSS filter list with each function's argument shape checked. The canvas
/// stores the serialized string; invalid values leave the previous filter
/// (<https://drafts.fxtf.org/filter-effects/#filter-property>).
fn valid_filter(text: &str) -> bool {
    let mut rest = text.trim();
    if rest.eq_ignore_ascii_case("none") {
        return true;
    }
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let name_length = rest
            .find(|ch: char| !(ch.is_ascii_alphabetic() || ch == '-'))
            .unwrap_or(rest.len());
        let (name, after_name) = rest.split_at(name_length);
        if name.is_empty() {
            return false;
        }
        let after_name = after_name.trim_start();
        let Some(after_open) = after_name.strip_prefix('(') else {
            return false;
        };
        let Some(close) = after_open.find(')') else {
            return false;
        };
        let argument = after_open[..close].trim();
        let valid = match name {
            "blur" => argument
                .strip_suffix("px")
                .and_then(|value| value.trim().parse::<f32>().ok())
                .is_some_and(|value| value.is_finite() && value >= 0.0),
            "brightness" | "contrast" | "grayscale" | "invert" | "opacity" | "saturate"
            | "sepia" => percentage_or_number(argument),
            "hue-rotate" => angle(argument),
            "drop-shadow" | "url" => !argument.is_empty(),
            _ => false,
        };
        if !valid {
            return false;
        }
        rest = after_open[close + 1..].trim_start();
    }
    true
}

fn percentage_or_number(text: &str) -> bool {
    let text = text.strip_suffix('%').unwrap_or(text).trim();
    text.parse::<f32>()
        .is_ok_and(|value| value.is_finite() && value >= 0.0)
}

fn angle(text: &str) -> bool {
    let number = ["deg", "rad", "turn", "grad"]
        .iter()
        .find_map(|unit| text.strip_suffix(unit))
        .unwrap_or(text)
        .trim();
    number.parse::<f32>().is_ok_and(f32::is_finite)
}

/// The document's Parley font context. It includes `@font-face` fonts, which
/// the shared process context does not.
fn document_font_context(ctx: &Ctx<'_>, object: &Object<'_>) -> Result<parley::FontContext> {
    let id = canvas_id(object)?;
    let world = world(ctx)?;
    let font_ctx = world
        .borrow()
        .document(id)
        .map(|parsed| parsed.document.base.font_context());
    font_ctx.map_or_else(
        || Ok(world.borrow().runtime.font_ctx.clone()),
        |font_ctx| {
            Ok(font_ctx
                .lock()
                .map(|font_ctx| font_ctx.clone())
                .unwrap_or_default())
        },
    )
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

fn canvas_object(this: This<Object<'_>>) -> Result<Value<'_>> {
    let id = canvas_id(&this.0)?;
    wrap_node(this.0.ctx(), id)
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
