#![expect(
    clippy::needless_pass_by_value,
    reason = "rquickjs passes every JS argument by value"
)]

//! Geometry interfaces used by canvas and CSSOM View.
//!
//! `DOMPoint` and `DOMPointReadOnly` are dictionaries with identity in
//! JavaScript; canvas consumes them as `DOMPointInit` when reading
//! `roundRect` radii
//! (<https://drafts.fxtf.org/geometry/#DOMPoint>,
//! <https://drafts.fxtf.org/geometry/#dictdef-dompointinit>).

use rquickjs::class::Trace;
use rquickjs::prelude::Opt;
use rquickjs::{Coerced, Ctx, Result};

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMPoint")]
pub(crate) struct JsDomPoint {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

#[rquickjs::methods]
impl JsDomPoint {
    #[qjs(constructor)]
    fn new(
        x: Opt<Coerced<f64>>,
        y: Opt<Coerced<f64>>,
        z: Opt<Coerced<f64>>,
        w: Opt<Coerced<f64>>,
    ) -> Self {
        Self {
            x: x.0.map_or(0.0, |value| *value),
            y: y.0.map_or(0.0, |value| *value),
            z: z.0.map_or(0.0, |value| *value),
            w: w.0.map_or(1.0, |value| *value),
        }
    }

    #[qjs(get)]
    fn x(&self) -> f64 {
        self.x
    }

    #[qjs(set, rename = "x")]
    fn set_x(&mut self, value: Coerced<f64>) {
        self.x = *value;
    }

    #[qjs(get)]
    fn y(&self) -> f64 {
        self.y
    }

    #[qjs(set, rename = "y")]
    fn set_y(&mut self, value: Coerced<f64>) {
        self.y = *value;
    }

    #[qjs(get)]
    fn z(&self) -> f64 {
        self.z
    }

    #[qjs(set, rename = "z")]
    fn set_z(&mut self, value: Coerced<f64>) {
        self.z = *value;
    }

    #[qjs(get)]
    fn w(&self) -> f64 {
        self.w
    }

    #[qjs(set, rename = "w")]
    fn set_w(&mut self, value: Coerced<f64>) {
        self.w = *value;
    }
}

#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "DOMPointReadOnly")]
pub(crate) struct JsDomPointReadOnly {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

#[rquickjs::methods]
impl JsDomPointReadOnly {
    #[qjs(constructor)]
    fn new(
        x: Opt<Coerced<f64>>,
        y: Opt<Coerced<f64>>,
        z: Opt<Coerced<f64>>,
        w: Opt<Coerced<f64>>,
    ) -> Self {
        Self {
            x: x.0.map_or(0.0, |value| *value),
            y: y.0.map_or(0.0, |value| *value),
            z: z.0.map_or(0.0, |value| *value),
            w: w.0.map_or(1.0, |value| *value),
        }
    }

    #[qjs(get)]
    fn x(&self) -> f64 {
        self.x
    }

    #[qjs(get)]
    fn y(&self) -> f64 {
        self.y
    }

    #[qjs(get)]
    fn z(&self) -> f64 {
        self.z
    }

    #[qjs(get)]
    fn w(&self) -> f64 {
        self.w
    }
}

pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let globals = ctx.globals();
    rquickjs::Class::<JsDomPoint>::define(&globals)?;
    rquickjs::Class::<JsDomPointReadOnly>::define(&globals)?;
    Ok(())
}
