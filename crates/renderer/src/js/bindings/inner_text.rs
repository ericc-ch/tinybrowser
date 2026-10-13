//! Realm wrapper for rendered-text collection.
//!
//! The collection algorithm lives in `crate::dom::text`; this module resolves
//! the calling realm's document and applies it.

use super::world_for_node;
use crate::dom::tree::NodeId;
use rquickjs::{Ctx, Result};

pub(crate) fn rendered_text(ctx: &Ctx<'_>, id: NodeId) -> Result<String> {
    let owner = world_for_node(ctx, id)?;
    let owner = owner.borrow();
    let Some(parsed) = owner.document(id) else {
        return Ok(String::new());
    };
    Ok(crate::dom::text::rendered_text(&parsed.document.base, id))
}
