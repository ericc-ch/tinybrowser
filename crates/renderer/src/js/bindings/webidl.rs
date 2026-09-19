//! `WebIDL` argument conversion helpers.

use super::{adopt_across_documents, host_node_id, throw_dom, throw_dom_error, world};
use rquickjs::function::Rest;

use dom::NodeId;

use rquickjs::{Ctx, Exception, Function, Object, Result, Value};

/// Dictionary member truthiness (`ToBoolean`, missing members are false).
pub(crate) fn option_truthy<'js>(ctx: &Ctx<'js>, options: &Object<'js>, key: &str) -> Result<bool> {
    let value: Value = options.get(key)?;
    if value.is_undefined() || value.is_null() {
        return Ok(false);
    }
    to_boolean(ctx, &value)
}

/// `WebIDL` `ToBoolean` (<https://webidl.spec.whatwg.org/#es-boolean>).
///
/// Primitives convert directly (no user code can run for them); anything else
/// goes through the pristine `Boolean`, captured at install. A page-assigned
/// `Boolean` global must not change the answer, and `0n` is the one falsy
/// bigint.
pub(crate) fn to_boolean<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    if let Some(boolean) = value.as_bool() {
        return Ok(boolean);
    }
    if value.is_null() || value.is_undefined() {
        return Ok(false);
    }
    if let Some(number) = value.as_number() {
        return Ok(number != 0.0 && !number.is_nan());
    }
    if let Some(string) = value.as_string() {
        return Ok(!string.to_string().is_ok_and(|string| string.is_empty()));
    }
    let world_rc = world(ctx)?;
    if let Some(boolean) = world_rc.borrow().pristine_boolean.clone() {
        let boolean: Function = boolean.restore(ctx)?;
        return boolean.call((value.clone(),));
    }
    // Install predates the capture: fall back to the (clobberable) global.
    let boolean: Function = ctx.globals().get("Boolean")?;
    boolean.call((value.clone(),))
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

/// `ToString` through the pristine `String`, captured at install: a
/// page-assigned global must not hijack `DOMString` conversion or re-enter
/// Rust through it.
pub(crate) fn webidl_to_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    if let Ok(world_rc) = world(ctx)
        && let Some(to_string) = world_rc.borrow().pristine_string.clone()
    {
        let to_string: Function = to_string.restore(ctx)?;
        let text: rquickjs::String = to_string.call((value,))?;
        return text.to_string();
    }
    // Install predates the capture: fall back to the (clobberable) global.
    let to_string: Function = ctx.globals().get("String")?;
    let text: rquickjs::String = to_string.call((value,))?;
    text.to_string()
}

/// [Converting nodes into a node](https://dom.spec.whatwg.org/#convert-nodes-into-a-node):
/// strings become `Text`, one node stays itself, several become a fragment.
/// Cross-document nodes adopt into the target document instead of throwing.
/// Strings convert before any DOM borrow is held: `ToString` runs page code,
/// which must not observe or re-enter a half-built fragment.
pub(crate) fn convert_nodes_into_node<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    nodes: Rest<Value<'js>>,
) -> Result<NodeId> {
    enum Piece {
        Node(NodeId),
        Text(String),
    }
    let mut pieces = Vec::with_capacity(nodes.0.len());
    for value in nodes.0 {
        if let Some(id) = host_node_id(ctx, &value) {
            pieces.push(Piece::Node(adopt_across_documents(ctx, document, id)?));
        } else {
            pieces.push(Piece::Text(webidl_to_string(ctx, value)?));
        }
    }
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    let dom = &mut parsed.dom;
    if pieces.len() == 1 {
        return match pieces.pop() {
            Some(Piece::Node(id)) => Ok(id),
            Some(Piece::Text(text)) => Ok(dom.create_text(text)),
            None => Err(Exception::throw_internal(ctx, "empty node list")),
        };
    }
    let fragment = dom.create_fragment();
    for piece in pieces {
        let id = match piece {
            Piece::Node(id) => id,
            Piece::Text(text) => dom.create_text(text),
        };
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
