//! `FontFace` host support.
//!
//! The JS shim fetches the font bytes and calls `__tbRegisterFont`; this
//! registers them with the document's Parley font context so canvas text
//! shapes with the same faces as page text
//! (<https://drafts.csswg.org/css-font-loading/#fontface-interface>).

use std::sync::Arc;

use rquickjs::{Ctx, Result, prelude::Func};

use crate::js::bindings::world;

pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
    ctx.globals()
        .set("__tbRegisterFont", Func::from(register_font))?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn register_font(ctx: Ctx<'_>, family: String, data: Vec<u8>) -> Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let world = world(&ctx)?;
    let mut world = world.borrow_mut();
    let override_name = parley::fontique::FontInfoOverride {
        family_name: Some(&family),
        ..Default::default()
    };
    // The active document owns the font context canvas text shapes with.
    if let Some(parsed) = world.main_document_mut() {
        let font_ctx = parsed.document.base.font_context();
        if let Ok(mut font_ctx) = font_ctx.lock() {
            font_ctx.collection.register_fonts(
                parley::fontique::Blob::new(Arc::new(data.clone())),
                Some(override_name),
            );
        }
    }
    // Detached canvases fall back to the runtime context, so register there
    // too.
    world.runtime.font_ctx.collection.register_fonts(
        parley::fontique::Blob::new(Arc::new(data)),
        Some(override_name),
    );
    Ok(())
}
