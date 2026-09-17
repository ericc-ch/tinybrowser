//! `WebIDL` argument conversion helpers.

use super::{host_node_id, throw_dom, throw_dom_error, world};
use rquickjs::function::Rest;

use dom::NodeId;

use rquickjs::{Ctx, Exception, Function, Object, Result, Value};

/// Dictionary member truthiness (`ToBoolean`, missing members are false).
pub(crate) fn option_truthy(options: &Object<'_>, key: &str) -> Result<bool> {
    let value: Value = options.get(key)?;
    if value.is_undefined() || value.is_null() {
        return Ok(false);
    }
    Ok(to_boolean(&value))
}

/// `WebIDL` `ToBoolean` (<https://webidl.spec.whatwg.org/#es-boolean>).
///
/// A direct ECMAScript conversion, not a call to the page's `Boolean`: the
/// global can be replaced by page script.
pub(crate) fn to_boolean(value: &Value<'_>) -> bool {
    if let Some(boolean) = value.as_bool() {
        return boolean;
    }
    if value.is_null() || value.is_undefined() {
        return false;
    }
    if let Some(number) = value.as_number() {
        return number != 0.0 && !number.is_nan();
    }
    if let Some(string) = value.as_string() {
        return !string.to_string().is_ok_and(|string| string.is_empty());
    }
    // Objects, symbols, and BigInts; `0n` is the one falsy BigInt.
    true
}

pub(crate) struct OptString(pub(crate) Option<String>);

/// `WebIDL` `DOMString` conversion: `ToString(value)`
/// (<https://webidl.spec.whatwg.org/#es-DOMString>).
pub(crate) struct WebIdlString(pub(crate) String);

/// `[LegacyNullToEmptyString]` `DOMString`: `null` becomes the empty string
/// (<https://webidl.spec.whatwg.org/#LegacyNullToEmptyString>).
pub(crate) struct LegacyNullString(pub(crate) String);

/// An optional title argument: omitted and `undefined` mean "not given",
/// `null` is the string "null" like any other `DOMString`.
pub(crate) struct OptionalTitle(pub(crate) Option<String>);

/// `WebIDL` `unsigned long` conversion
/// (<https://webidl.spec.whatwg.org/#es-unsigned-long>).
pub(crate) struct WebIdlUnsignedLong(pub(crate) u32);

/// `ToString` without a raw conversion API: call the global `String`.
pub(crate) fn webidl_to_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    let to_string: Function = ctx.globals().get("String")?;
    let text: rquickjs::String = to_string.call((value,))?;
    text.to_string()
}

/// [Converting nodes into a node](https://dom.spec.whatwg.org/#convert-nodes-into-a-node):
/// strings become `Text`, one node stays itself, several become a fragment.
pub(crate) fn convert_nodes_into_node<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    nodes: Rest<Value<'js>>,
) -> Result<NodeId> {
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let dom = &mut parsed.dom;
    let mut ids: Vec<NodeId> = Vec::with_capacity(nodes.0.len());
    for value in nodes.0 {
        if let Some(id) = host_node_id(ctx, &value) {
            if id.document_id() != document.document_id() {
                return Err(throw_dom(
                    ctx,
                    "HierarchyRequestError",
                    "nodes belong to different documents",
                ));
            }
            ids.push(id);
        } else {
            let text = webidl_to_string(ctx, value)?;
            ids.push(dom.create_text(text));
        }
    }
    if ids.len() == 1 {
        return Ok(ids[0]);
    }
    let fragment = dom.create_fragment();
    for id in ids {
        dom.append(fragment, id)
            .map_err(|err| throw_dom_error(ctx, err))?;
    }
    Ok(fragment)
}

/// A selector syntax error is a `SyntaxError` `DOMException`
/// (<https://dom.spec.whatwg.org/#scope-match-a-selectors-string>).
pub(crate) fn select_error(ctx: &Ctx<'_>, err: &dom::SelectError) -> rquickjs::Error {
    match err {
        dom::SelectError::Syntax(_) => throw_dom(ctx, "SyntaxError", &err.to_string()),
        _ => Exception::throw_internal(ctx, &err.to_string()),
    }
}

/// Integer conversion from a `double`
/// (<https://webidl.spec.whatwg.org/#abstract-opdef-converttoint>): NaN and
/// infinities become 0, then the truncated value wraps modulo 2^32.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "`rem_euclid` returns a value in [0, 2^32), so the cast is exact and non-negative"
)]
pub(crate) fn webidl_unsigned_long(number: f64) -> u32 {
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    let modulo = number.trunc().rem_euclid(4_294_967_296.0);
    modulo as u32
}
