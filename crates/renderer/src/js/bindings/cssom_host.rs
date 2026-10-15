//! Host calls that expose a document's parsed stylesheets as CSSOM rule lists.
//!
//! The style engine already keeps a `CSSRuleList` per `<style>` and
//! `<link rel=stylesheet>` owner
//! (<https://drafts.csswg.org/cssom/#concept-css-rule-list>). These functions
//! are the only bridge: `cssom.js` builds the live list objects on top.

use blitz_dom::{CssRuleInfo, CssomError};
use rquickjs::{Ctx, Exception, Object, Result, Value};

use super::{host_node_id, throw_dom, world};

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let bridge = crate::js::bridge::object(ctx)?;
    bridge.set(
        "__tb_sheet_rule_count",
        rquickjs::prelude::Func::from(sheet_rule_count),
    )?;
    bridge.set(
        "__tb_sheet_rule_info",
        rquickjs::prelude::Func::from(sheet_rule_info),
    )?;
    bridge.set(
        "__tb_sheet_style_css_text",
        rquickjs::prelude::Func::from(sheet_style_css_text),
    )?;
    bridge.set(
        "__tb_sheet_insert_rule",
        rquickjs::prelude::Func::from(sheet_insert_rule),
    )?;
    bridge.set(
        "__tb_sheet_delete_rule",
        rquickjs::prelude::Func::from(sheet_delete_rule),
    )?;
    Ok(())
}

/// `path` is a comma-separated list of rule-list indexes, empty for the
/// stylesheet's own list
/// (<https://drafts.csswg.org/cssom/#concept-css-rule-list>).
fn rule_path(path: &str) -> Vec<usize> {
    if path.is_empty() {
        return Vec::new();
    }
    path.split(',')
        .filter_map(|part| part.parse::<usize>().ok())
        .collect()
}

fn cssom_throw(ctx: &Ctx<'_>, err: CssomError) -> rquickjs::Error {
    throw_dom(ctx, err.dom_exception_name(), "CSSOM operation failed")
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn sheet_rule_count<'js>(ctx: Ctx<'js>, element: Value<'js>, path: String) -> f64 {
    let Ok(world) = world(&ctx) else {
        return -1.0;
    };
    let Some(id) = host_node_id(&ctx, &element) else {
        return -1.0;
    };
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return -1.0;
    };
    let path = rule_path(&path);
    parsed
        .document
        .base
        .stylesheet_rule_count(id.node, &path)
        .map_or(-1.0, |count| f64::from(u32::try_from(count).unwrap_or(u32::MAX)))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn sheet_rule_info<'js>(ctx: Ctx<'js>, element: Value<'js>, path: String) -> Result<Value<'js>> {
    let world = world(&ctx)?;
    let Some(id) = host_node_id(&ctx, &element) else {
        return Ok(Value::new_null(ctx));
    };
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let path = rule_path(&path);
    let Some(info) = parsed.document.base.stylesheet_rule_info(id.node, &path) else {
        return Ok(Value::new_null(ctx.clone()));
    };
    rule_info_object(&ctx, info).map(Value::from)
}

fn rule_info_object<'js>(ctx: &Ctx<'js>, info: CssRuleInfo) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("interface", info.interface)?;
    object.set("type", info.rule_type)?;
    object.set("cssText", info.css_text)?;
    object.set("hasChildRules", info.has_child_rules)?;
    object.set("hasStyle", info.has_style)?;
    for (name, value) in info.attributes {
        object.set(name, value)?;
    }
    Ok(object)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn sheet_style_css_text<'js>(ctx: Ctx<'js>, element: Value<'js>, path: String) -> String {
    let Ok(world) = world(&ctx) else {
        return String::new();
    };
    let Some(id) = host_node_id(&ctx, &element) else {
        return String::new();
    };
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return String::new();
    };
    let path = rule_path(&path);
    parsed
        .document
        .base
        .stylesheet_rule_style_css_text(id.node, &path)
        .unwrap_or_default()
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn sheet_insert_rule<'js>(
    ctx: Ctx<'js>,
    element: Value<'js>,
    path: String,
    rule: String,
    index: f64,
) -> Result<f64> {
    let world = world(&ctx)?;
    let Some(id) = host_node_id(&ctx, &element) else {
        return Err(Exception::throw_type(&ctx, "not a stylesheet owner"));
    };
    let index = rule_index(index);
    let path = rule_path(&path);
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Err(Exception::throw_type(&ctx, "no document"));
    };
    match parsed
        .document
        .base
        .stylesheet_insert_rule(id.node, &path, &rule, index)
    {
        Ok(inserted) => Ok(f64::from(u32::try_from(inserted).unwrap_or(u32::MAX))),
        Err(err) => Err(cssom_throw(&ctx, err)),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn sheet_delete_rule<'js>(ctx: Ctx<'js>, element: Value<'js>, path: String, index: f64) -> Result<()> {
    let world = world(&ctx)?;
    let Some(id) = host_node_id(&ctx, &element) else {
        return Err(Exception::throw_type(&ctx, "not a stylesheet owner"));
    };
    let index = rule_index(index);
    let path = rule_path(&path);
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(id) else {
        return Err(Exception::throw_type(&ctx, "no document"));
    };
    parsed
        .document
        .base
        .stylesheet_delete_rule(id.node, &path, index)
        .map_err(|err| cssom_throw(&ctx, err))
}

fn rule_index(index: f64) -> usize {
    if !index.is_finite() || index < 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        index as usize
    }
}
