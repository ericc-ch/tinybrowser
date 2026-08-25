//! Element states a headless tree can answer truthfully.
//!
//! Every selector-engine state pseudo-class bottoms out here: Servo's
//! `selectors` parses the *name* and calls back, this module owns the
//! *meaning* against our arena. Grouped by the spec section that defines
//! each state, so fidelity gaps are visible as missing functions rather
//! than scattered strings.
//!
//! Truth policy (audit findings M3/L7): a state is implemented when static
//! markup fully determines it (possibly as a documented subset of its full
//! runtime semantics), answers `false` vacuously when the needed context
//! cannot exist here (no pointer, no history), and never invents semantics
//! beyond what the cited section defines. Where engines genuinely disagree
//! or a clause is deferred, the cut is stated in the function's doc; those
//! notes feed the audit trail, so a future reader inherits an accurate map
//! rather than an optimistic one.
//!
//! Name and attribute lookups also live here ([`local_is`],
//! [`attr_value`], [`is_html`]): one definition of the case-regime policy,
//! shared with selector-side name matching in `select.rs`.

use crate::arena::Dom;
use crate::id::NodeId;
use crate::node::{NodeKind, QualName, html_namespace};

// ── shared lookups ──────────────────────────────────────────────────────────

/// Qualified name of a live element, else `None`.
fn qual_name(dom: &Dom, id: NodeId) -> Option<&QualName> {
    match dom.get(id)?.kind() {
        NodeKind::Element { name, .. } => Some(name),
        _ => None,
    }
}

/// Whether the element lives in the HTML namespace: the case-regime switch
/// shared with selector name matching in `select.rs`.
pub(crate) fn is_html(dom: &Dom, id: NodeId) -> bool {
    qual_name(dom, id).is_some_and(|name| name.ns == html_namespace())
}

/// Whether any listed local name matches the element under its case regime:
/// ASCII-insensitive for HTML elements, exact elsewhere, so hand-built
/// `<INPUT>` behaves like tokenized `<input>`. This is the single
/// definition of that policy; selector-side name checks route here too.
pub(crate) fn local_is(dom: &Dom, id: NodeId, names: &[&str]) -> bool {
    let Some(name) = qual_name(dom, id) else {
        return false;
    };
    let html = is_html(dom, id);
    let local = name.local.as_ref();
    names.iter().any(|wanted| {
        if html {
            local.eq_ignore_ascii_case(wanted)
        } else {
            local == *wanted
        }
    })
}

/// First no-namespace attribute whose local name equals `name` under the
/// element's case regime. Namespace-restricted on purpose: `class`, `id`,
/// `href`, and every other selector-visible attribute live in the empty
/// namespace (legacy `xlink:href` deliberately does not count; modern SVG2
/// dropped it, and one lookup policy keeps `[href]` and `:link` answers
/// consistent).
pub(crate) fn attr_value<'a>(dom: &'a Dom, id: NodeId, name: &str) -> Option<&'a str> {
    let NodeKind::Element { attributes, .. } = dom.get(id)?.kind() else {
        return None;
    };
    let html = is_html(dom, id);
    attributes.iter().find_map(|attribute| {
        if !attribute.name.ns.is_empty() {
            return None;
        }
        let stored = attribute.name.local.as_ref();
        let named = if html {
            stored.eq_ignore_ascii_case(name)
        } else {
            stored == name
        };
        named.then_some(attribute.value.as_str())
    })
}

/// ASCII-case-insensitive equality: CSS keywords and the attribute values
/// compared in these positions are case-insensitive.
fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// One RFC 4647 §3.3.2 *extended filtering* comparison, lowercased on both
/// sides: range subtags correspond positionally to tag subtags, `*` matches
/// exactly one subtag, and tag subtags beyond the range's length are free
/// specificity. Compares whole subtags only (no byte slicing anywhere),
/// so arbitrary (even malformed) markup cannot panic this matcher.
///
/// ```text
/// range "en"      matches "en", "en-US", "en-Latn-US"
/// range "en-Latn" matches "en-Latn", "en-Latn-US"; not "en"
/// range "*-Cyrl"  matches "sr-Cyrl"; not "sr" (`*` consumes exactly one)
/// ```
fn lang_range_matches(range: &str, tag: &str) -> bool {
    let range = range.to_ascii_lowercase();
    if range == "*" {
        // The bare wildcard matches everything, per the RFC's special case.
        return true;
    }
    let tag = tag.to_ascii_lowercase();
    let mut tag_subtags = tag.split('-');
    for range_subtag in range.split('-') {
        match tag_subtags.next() {
            // Tag ran out first: it is less specific than the range demands.
            None => return false,
            Some(tag_subtag) => {
                if range_subtag != "*" && range_subtag != tag_subtag {
                    return false;
                }
            }
        }
    }
    true
}

// ── hyperlinks ──────────────────────────────────────────────────────────────

/// `:link` / `:any-link`, an element that is a hyperlink: local name
/// `a`, `area`, or `link` carrying an `href`, in **any** namespace.
///
/// <https://drafts.csswg.org/selectors-4/#selectordef-link> defines the
/// match by element *type* and href, not by namespace; SVG2 gives `<a>`
/// hyperlink status the same way
/// (<https://svgwg.org/svg2-draft/struct.html#__svg__SVGElementElement>),
/// which is why the old html-only gate was dropped (audit finding L9).
pub(crate) fn is_hyperlink(dom: &Dom, id: NodeId) -> bool {
    local_is(dom, id, &["a", "area", "link"]) && attr_value(dom, id, "href").is_some()
}

// ── form-control UI states ──────────────────────────────────────────────────

/// The disableable population: button, input, select, textarea, optgroup,
/// option, fieldset (<https://html.spec.whatwg.org/#concept-fe-disabled>;
/// form-associated custom elements cannot exist here). Also the population
/// [`is_enabled`] ranges over.
fn is_form_control(dom: &Dom, id: NodeId) -> bool {
    local_is(
        dom,
        id,
        &[
            "button", "input", "select", "textarea", "optgroup", "option", "fieldset",
        ],
    )
}

/// `:disabled` / actually-disabled per HTML §4.15
/// (<https://html.spec.whatwg.org/#selector-disabled>): a form control is
/// disabled when it carries `disabled`; or when a *disabled* `fieldset` is
/// among its ancestors (every disableable kind inherits that, not just
/// options); or, for `option`/`optgroup`, when its nearest ancestor
/// `select` is disabled; or, for an `option`, when its direct parent
/// `optgroup` is disabled (§4.10.11).
///
/// Accepted approximation: the spec exempts descendants of a disabled
/// fieldset's *first* `legend` child; that carve-out is not modeled. No
/// agent-facing behavior depends on it before the js layer lands.
pub(crate) fn is_disabled(dom: &Dom, id: NodeId) -> bool {
    if !is_form_control(dom, id) {
        return false;
    }
    if attr_value(dom, id, "disabled").is_some() {
        return true;
    }
    // option/optgroup additionally answer to their select's disability.
    if local_is(dom, id, &["option", "optgroup"]) {
        let mut cursor = dom.parent(id);
        while let Some(ancestor) = cursor {
            if local_is(dom, ancestor, &["select"]) {
                if attr_value(dom, ancestor, "disabled").is_some() {
                    return true;
                }
                break; // nearest select decides; further ancestors don't
            }
            cursor = dom.parent(ancestor);
        }
    }
    // …and an `option` answers to a directly enclosing disabled `optgroup`
    // (§4.10.11: disabled "if it is a child of an optgroup that has a
    // disabled attribute"), the inheritance channel beside the select
    // above and the fieldset below. Direct parent only, per the spec's
    // wording.
    if local_is(dom, id, &["option"])
        && dom.parent(id).is_some_and(|group| {
            local_is(dom, group, &["optgroup"]) && attr_value(dom, group, "disabled").is_some()
        })
    {
        return true;
    }
    // …and every form control answers to a disabled fieldset ancestor.
    let mut cursor = dom.parent(id);
    while let Some(ancestor) = cursor {
        if local_is(dom, ancestor, &["fieldset"]) && attr_value(dom, ancestor, "disabled").is_some()
        {
            return true;
        }
        cursor = dom.parent(ancestor);
    }
    false
}

/// `:enabled`, the negation of [`is_disabled`] *among disableable
/// elements* (<https://html.spec.whatwg.org/#selector-enabled>): a `div`
/// without `disabled` is not "enabled", it is out of scope.
pub(crate) fn is_enabled(dom: &Dom, id: NodeId) -> bool {
    is_form_control(dom, id) && !is_disabled(dom, id)
}

/// Whether an `option`'s specified selectedness is true, the static part
/// of selectedness
/// (<https://html.spec.whatwg.org/#concept-option-selectedness>): the
/// `selected` content attribute sets it.
fn has_selected_attribute(dom: &Dom, id: NodeId) -> bool {
    attr_value(dom, id, "selected").is_some()
}

/// Nearest `select` above `id`, if one exists: the owner whose list of
/// options decides default selectedness (`id` is an `option` here, never
/// the select itself).
fn owning_select(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let mut cursor = dom.parent(id);
    while let Some(ancestor) = cursor {
        if local_is(dom, ancestor, &["select"]) {
            return Some(ancestor);
        }
        cursor = dom.parent(ancestor);
    }
    None
}

/// Every descendant `option` of `root`, in tree order: HTML's "list of
/// options", within which `optgroup`s (and anything else wrapping them)
/// are transparent containers.
fn collect_descendant_options(dom: &Dom, root: NodeId, options: &mut Vec<NodeId>) {
    if let Some(kids) = dom.children(root) {
        for kid in kids {
            if local_is(dom, *kid, &["option"]) {
                options.push(*kid);
            }
            collect_descendant_options(dom, *kid, options);
        }
    }
}

/// `:checked` per HTML §4.16.3 (<https://html.spec.whatwg.org/#selector-checked>):
/// checkbox/radio inputs whose checkedness is true (statically, those
/// carrying `checked`), or options whose selectedness is true. Selectedness
/// has a static default (in a select without `multiple`, the first option
/// of its list of options is selected when nothing in that list carries
/// `selected`, and the list flattens `optgroup`s), so fresh parsed pages
/// answer exactly as browsers do (subagent review R3-4).
pub(crate) fn is_checked(dom: &Dom, id: NodeId) -> bool {
    if local_is(dom, id, &["input"]) {
        let ty = attr_value(dom, id, "type").unwrap_or("text");
        return (eq_ignore_case(ty, "checkbox") || eq_ignore_case(ty, "radio"))
            && attr_value(dom, id, "checked").is_some();
    }
    if local_is(dom, id, &["option"]) {
        if has_selected_attribute(dom, id) {
            return true;
        }
        // Default selectedness: first option of a non-multiple select whose
        // list carries no `selected` at all.
        let Some(select) = owning_select(dom, id) else {
            return false;
        };
        if attr_value(dom, select, "multiple").is_some() {
            return false;
        }
        let mut options = Vec::new();
        collect_descendant_options(dom, select, &mut options);
        let Some((first, rest)) = options.split_first() else {
            return false;
        };
        return *first == id
            && rest
                .iter()
                .all(|&option| !has_selected_attribute(dom, option));
    }
    false
}

/// The constraint-validation population: `input`, `select`, `textarea`
/// (<https://html.spec.whatwg.org/#selector-required>).
fn constraint_target(dom: &Dom, id: NodeId) -> bool {
    local_is(dom, id, &["input", "select", "textarea"])
}

/// `:required` (<https://html.spec.whatwg.org/#selector-required>).
pub(crate) fn is_required(dom: &Dom, id: NodeId) -> bool {
    constraint_target(dom, id) && attr_value(dom, id, "required").is_some()
}

/// `:optional`: the same population without `required`.
pub(crate) fn is_optional(dom: &Dom, id: NodeId) -> bool {
    constraint_target(dom, id) && attr_value(dom, id, "required").is_none()
}

/// `:read-only` per HTML §4.16.3 as implemented by Chrome: an
/// `input`/`textarea` is read-only when `readonly` or `disabled`; other
/// elements are neither read-only nor read-write. (The section's other
/// reading, Firefox makes *all* non-editable elements `:read-only`, is
/// the documented counter-engine; `contenteditable` hosts have no
/// representation in this tree yet.)
pub(crate) fn is_read_only(dom: &Dom, id: NodeId) -> bool {
    if !local_is(dom, id, &["input", "textarea"]) {
        return false;
    }
    attr_value(dom, id, "readonly").is_some() || attr_value(dom, id, "disabled").is_some()
}

/// `:read-write`, an editable control: the same `input`/`textarea`
/// population that is not [`is_read_only`]. Non-form elements match neither
/// state, following the Chrome reading above.
pub(crate) fn is_read_write(dom: &Dom, id: NodeId) -> bool {
    local_is(dom, id, &["input", "textarea"]) && !is_read_only(dom, id)
}

/// Input types that can present a placeholder
/// (<https://html.spec.whatwg.org/#attr-input-placeholder>: textual and
/// numeric-entry types only; a checkbox shows nothing).
fn placeholder_capable_type(dom: &Dom, id: NodeId) -> bool {
    let ty = attr_value(dom, id, "type").unwrap_or("text");
    matches!(
        ty.to_ascii_lowercase().as_str(),
        "text" | "search" | "url" | "tel" | "email" | "password" | "number"
    )
}

/// `:placeholder-shown`: a placeholder is *shown* only while the control's
/// value is empty (<https://html.spec.whatwg.org/#attr-input-placeholder>).
/// For an `input` that means a placeholder-capable type with an absent or
/// empty `value`; for a `textarea` the value *is* its text content, so any
/// non-empty text hides it.
pub(crate) fn is_placeholder_shown(dom: &Dom, id: NodeId) -> bool {
    if attr_value(dom, id, "placeholder").is_none() {
        return false;
    }
    if local_is(dom, id, &["input"]) {
        if !placeholder_capable_type(dom, id) {
            return false;
        }
        return attr_value(dom, id, "value").is_none_or(str::is_empty);
    }
    if local_is(dom, id, &["textarea"]) {
        let mut empty = true;
        if let Some(kids) = dom.children(id) {
            for kid in kids {
                if let Some(NodeKind::Text { data }) = dom.get(*kid).map(|view| view.kind()) {
                    empty &= data.is_empty();
                }
            }
        }
        return empty;
    }
    false
}

/// `:default`, static subset of <https://html.spec.whatwg.org/#selector-default>:
/// controls whose *default* checkedness/selectedness is true (checkbox/
/// radio inputs carrying `checked`, options carrying `selected`). The other
/// default clause, the first submit button of a form being its default
/// button, needs the form-owner association no tree stores yet; until the
/// forms model lands, such buttons do not match.
pub(crate) fn is_default(dom: &Dom, id: NodeId) -> bool {
    if local_is(dom, id, &["input"]) {
        let ty = attr_value(dom, id, "type").unwrap_or("text");
        return (eq_ignore_case(ty, "checkbox") || eq_ignore_case(ty, "radio"))
            && attr_value(dom, id, "checked").is_some();
    }
    local_is(dom, id, &["option"]) && has_selected_attribute(dom, id)
}

/// `:indeterminate`, static subset: a `progress` element without a `value`
/// attribute (<https://html.spec.whatwg.org/#the-progress-element>: "the
/// progress bar is indeterminate … when the element has no value
/// attribute"). Radio groups also feed this state, but group membership is
/// scoped by form owner (an association no tree stores yet), so radios
/// stay deferred until the forms model lands.
pub(crate) fn is_indeterminate(dom: &Dom, id: NodeId) -> bool {
    local_is(dom, id, &["progress"]) && attr_value(dom, id, "value").is_none()
}

/// `:defined` per <https://html.spec.whatwg.org/#selector-defined>: an
/// element is undefined when it is a valid-but-unregistered *custom
/// element*, an HTML-ns name containing `-` that is not one of the
/// reserved hyphenated names. That set is fully static here (no registry
/// can exist), so the truth is representable and represented (subagent
/// review R3-9).
pub(crate) fn is_defined(dom: &Dom, id: NodeId) -> bool {
    const RESERVED: &[&str] = &[
        "annotation-xml",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    let Some(name) = qual_name(dom, id) else {
        return false;
    };
    if name.ns != html_namespace() {
        return true; // foreign elements are always defined
    }
    let local = name.local.as_ref();
    if !local.contains('-') {
        return true;
    }
    // Hyphenated HTML name: defined only when it is one of the reserved
    // legacy names; anything else is an unregistered custom element.
    RESERVED
        .iter()
        .any(|reserved| local.eq_ignore_ascii_case(reserved))
}

// ── inherited document-language states ──────────────────────────────────────

/// `:lang(range…)`: the element's language is set by the nearest
/// ancestor-or-self carrying `lang` (HTML inheritance,
/// <https://html.spec.whatwg.org/#the-lang-attribute>); each comma-separated
/// range matches under RFC 4647 §3.3.2 extended filtering
/// (<https://drafts.csswg.org/selectors-4/#lang-pseudo>); see
/// [`lang_range_matches`] for the exact algorithm, including wildcards.
///
/// Only the literal `lang` attribute feeds inheritance today; `xml:lang`
/// and HTTP `Content-Language` defaults are unrepresented (documented cut,
/// revisit with the net layer).
pub(crate) fn lang_matches(dom: &Dom, id: NodeId, ranges: &[Box<str>]) -> bool {
    let mut found: Option<&str> = None;
    let mut cursor = Some(id);
    while let Some(current) = cursor {
        if let Some(value) = attr_value(dom, current, "lang") {
            found = Some(value);
            break;
        }
        cursor = dom.parent(current);
    }
    let Some(tag) = found else {
        return false;
    };
    ranges.iter().any(|range| lang_range_matches(range, tag))
}

/// The effective `dir` attribute value on one element, honoring the HTML
/// rules that matter statically: only `ltr`/`rtl` count; anything else
/// (`auto`, garbage, foreign elements) leaves the direction undefined here
/// and lets inheritance continue past this node.
fn dir_attr(dom: &Dom, id: NodeId) -> Option<&str> {
    if !is_html(dom, id) {
        return None;
    }
    attr_value(dom, id, "dir")
        .filter(|value| eq_ignore_case(value, "ltr") || eq_ignore_case(value, "rtl"))
}

/// `:dir(direction)`: nearest HTML ancestor-or-self with a `dir` attribute
/// of `ltr`/`rtl` (<https://drafts.csswg.org/selectors-4/#dir-pseudo>).
/// Defaults to `ltr`, the root default for HTML documents. Two documented
/// cuts: `dir="auto"` needs first-strong-character scanning (a static pass,
/// but one needing Unicode bidi classification; deferred, not a layout
/// feature; subagent review R3-20), and invalid `dir` values fall through
/// to inheritance per the spec's Undefined-direction state.
pub(crate) fn direction_is(dom: &Dom, id: NodeId, want: &str) -> bool {
    let mut cursor = Some(id);
    while let Some(current) = cursor {
        if let Some(found) = dir_attr(dom, current) {
            return eq_ignore_case(found, want);
        }
        cursor = dom.parent(current);
    }
    // No effective direction in the chain: the document default.
    eq_ignore_case(want, "ltr")
}
