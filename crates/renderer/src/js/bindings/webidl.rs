//! `WebIDL` argument conversion helpers.

use super::{adopt_across_documents, throw_dom, world};

use crate::js::world::{JournalEntry, NodeId};

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

/// A `DOMString` argument that keeps every UTF-16 code unit.
///
/// Character data may contain unpaired surrogates, which a Rust `String`
/// cannot hold, so these entry points carry the exact code units instead.
pub(crate) struct WebIdlCodeUnits(pub(crate) crate::dom_string::DomString);

/// `WebIDL` `unsigned long` conversion
/// (<https://webidl.spec.whatwg.org/#es-unsigned-long>).
pub(crate) struct WebIdlUnsignedLong(pub(crate) u32);

/// `DOMString` conversion through the engine's `ToString` operation
/// (<https://webidl.spec.whatwg.org/#es-DOMString>).
pub(crate) fn webidl_to_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String> {
    webidl_to_js_string(ctx, value)?.to_string()
}

/// `DOMString` conversion keeping every UTF-16 code unit
/// (<https://webidl.spec.whatwg.org/#es-DOMString>), for character data that
/// may contain unpaired surrogates.
pub(crate) fn webidl_to_units<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Vec<u16>> {
    webidl_to_js_string(ctx, value)?.to_utf16()
}

fn webidl_to_js_string<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<rquickjs::String<'js>> {
    let converted: rquickjs::Coerced<rquickjs::String> = rquickjs::FromJs::from_js(ctx, value)?;
    Ok(converted.0)
}

/// One already-converted `(Node or DOMString)` union member: the generated
/// union enums map into this before insertion.
pub(crate) enum NodeOrString<'js> {
    Node(super::host::NodeReference),
    String(rquickjs::String<'js>),
}

/// [Converting nodes into a node](https://dom.spec.whatwg.org/#convert-nodes-into-a-node)
/// over generated union members. Dispatch already ran the union conversion,
/// so strings arrive converted and nodes arrive as references.
pub(crate) fn convert_union_nodes_into_node<'js>(
    ctx: &Ctx<'js>,
    document: NodeId,
    nodes: Vec<NodeOrString<'js>>,
) -> Result<NodeId> {
    let mut pieces = Vec::with_capacity(nodes.len());
    for node in nodes {
        match node {
            NodeOrString::Node(reference) => {
                let Some(id) = reference.tree() else {
                    // The union trial only admits tree nodes, matching
                    // insertion below, which rejects attribute references.
                    return Err(throw_dom(ctx, "HierarchyRequestError", "attributes cannot be inserted"));
                };
                pieces.push(Piece::Node(id));
            }
            NodeOrString::String(string) => {
                pieces.push(Piece::Text(string.to_string()?));
            }
        }
    }
    assemble_nodes_into_node(ctx, document, pieces)
}

/// A converted node list piece: a tree node or text to create.
enum Piece {
    Node(NodeId),
    Text(String),
}

/// Phase two of node conversion: adopt nodes across documents, then return
/// the single node or a fragment holding them all.
fn assemble_nodes_into_node(
    ctx: &Ctx<'_>,
    document: NodeId,
    mut pieces: Vec<Piece>,
) -> Result<NodeId> {
    // Phase two, adoptions.
    for piece in &mut pieces {
        if let Piece::Node(id) = piece {
            *id = adopt_across_documents(ctx, document, *id)?;
        }
    }
    let world = world(ctx)?;
    let world = world.borrow();
    let Some(mut parsed) = world.document_mut(document) else {
        return Err(Exception::throw_type(ctx, "no document"));
    };
    if pieces.len() == 1 {
        return match pieces.pop() {
            Some(Piece::Node(id)) => Ok(id),
            Some(Piece::Text(text)) => {
                let node = parsed.document.base.mutate().create_text_node(&text);
                Ok(NodeId {
                    document: document.document,
                    node,
                })
            }
            None => Err(Exception::throw_internal(ctx, "empty node list")),
        };
    }
    let fragment = NodeId {
        document: document.document,
        node: parsed.document.create_fragment(),
    };
    for piece in pieces {
        let id = match piece {
            Piece::Node(id) => id,
            Piece::Text(text) => {
                let node = parsed
                    .document
                    .base
                    .mutate()
                    .create_text_node(&text);
                NodeId {
                    document: document.document,
                    node,
                }
            }
        };
        let previous = parsed
            .document
            .base
            .get_node(fragment.node)
            .and_then(|node| node.children.last().copied())
            .map(|node| NodeId {
                document: document.document,
                node,
            });
        parsed
            .document
            .base
            .mutate()
            .append_children(fragment.node, &[id.node]);
        parsed.document.record(JournalEntry::ChildList {
            target: fragment,
            added: vec![id],
            removed: Vec::new(),
            previous,
            next: None,
        });
    }
    Ok(fragment)
}

/// A selector syntax error is a `SyntaxError` `DOMException`
/// (<https://dom.spec.whatwg.org/#scope-match-a-selectors-string>).
pub(crate) fn select_error(ctx: &Ctx<'_>, err: &style_traits::ParseError<'_>) -> rquickjs::Error {
    throw_dom(ctx, "SyntaxError", &format!("{err:?}"))
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
