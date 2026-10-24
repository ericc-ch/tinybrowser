//! `FontFace` host support.
//!
//! The JS shim fetches the font bytes and calls `__tbRegisterFont` from
//! `FontFaceSet.add`; this registers them with the document's Parley font
//! context so canvas text shapes with the same faces as page text. `delete`
//! and `clear` call `__tbUnregisterFont` with the token returned here
//! (<https://drafts.csswg.org/css-font-loading/#fontface-interface>).

use std::sync::Arc;

use rquickjs::{Ctx, Result, prelude::Func};

use crate::js::bindings::world;
use crate::js::world::{FontFamilyEntry, FontRegistration};

pub(crate) fn install(ctx: &Ctx<'_>) -> Result<()> {
    ctx.globals()
        .set("__tbRegisterFont", Func::from(register_font))?;
    ctx.globals()
        .set("__tbUnregisterFont", Func::from(unregister_font))?;
    Ok(())
}

type Registered = Vec<FontFamilyEntry>;

fn descriptors(
    added: Vec<(parley::fontique::FamilyId, Vec<parley::fontique::FontInfo>)>,
) -> Registered {
    added
        .into_iter()
        .map(|(family, fonts)| {
            (
                family,
                fonts
                    .into_iter()
                    .map(|font| (font.width(), font.style(), font.weight()))
                    .collect(),
            )
        })
        .collect()
}

fn register_font(ctx: Ctx<'_>, family: String, data: Vec<u8>) -> Result<u32> {
    if data.is_empty() {
        return Ok(0);
    }
    let world = world(&ctx)?;
    let mut world = world.borrow_mut();
    let override_name = parley::fontique::FontInfoOverride {
        family_name: Some(&family),
        ..Default::default()
    };
    let mut fonts = Registered::new();
    let document = {
        let Some(parsed) = world.main_document_mut() else {
            return Ok(0);
        };
        let document = parsed.id;
        let font_ctx = parsed.document.base.font_context();
        if let Ok(mut font_ctx) = font_ctx.lock() {
            fonts.extend(descriptors(font_ctx.collection.register_fonts(
                parley::fontique::Blob::new(Arc::new(data.clone())),
                Some(override_name),
            )));
        }
        Some(document)
    };
    // Detached canvases fall back to the runtime context, so register there
    // too.
    fonts.extend(descriptors(
        world.runtime.font_ctx.collection.register_fonts(
            parley::fontique::Blob::new(Arc::new(data)),
            Some(override_name),
        ),
    ));
    let token = world.next_font_registration;
    world.next_font_registration = world.next_font_registration.wrapping_add(1);
    world
        .font_registrations
        .insert(token, FontRegistration { document, fonts });
    Ok(token)
}

fn unregister_font(ctx: Ctx<'_>, token: u32) -> Result<()> {
    let world = world(&ctx)?;
    let mut world = world.borrow_mut();
    let Some(registration) = world.font_registrations.remove(&token) else {
        return Ok(());
    };
    if let Some(document) = registration.document
        && let Some(parsed) = world.document_mut_by_id(document)
    {
        let font_ctx = parsed.document.base.font_context();
        if let Ok(mut font_ctx) = font_ctx.lock() {
            unregister(&mut font_ctx.collection, &registration.fonts);
        }
    }
    unregister(&mut world.runtime.font_ctx.collection, &registration.fonts);
    Ok(())
}

fn unregister(collection: &mut parley::fontique::Collection, fonts: &[FontFamilyEntry]) {
    for (family, entries) in fonts {
        for (width, style, weight) in entries {
            collection.unregister_font(*family, *width, *style, *weight);
        }
    }
}
