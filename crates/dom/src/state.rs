//! Element states a headless tree can answer truthfully.
//!
//! Every selector-engine state pseudo-class bottoms out here: Servo's
//! `selectors` parses the *name* and calls back, this module owns the
//! *meaning* against our arena. Grouped by the spec section that defines
//! each state, so fidelity gaps are visible as missing functions rather
//! than scattered strings.
//!
//! Truth policy: a state is implemented when static markup fully determines
//! it (possibly as a documented subset of its full runtime semantics),
//! answers `false` vacuously when the needed context cannot exist here (no
//! pointer, no history), and never invents semantics beyond what the cited
//! section defines. Where engines disagree or a clause is a static subset,
//! the cut is stated in the function's doc.
//!
//! Name and attribute lookups also live here ([`local_is`],
//! [`attr_value`], [`is_html`]): one definition of the case-regime policy,
//! shared with selector-side name matching in `select.rs`.

use crate::arena::Dom;
use crate::id::NodeId;
use crate::node::{QualName, html_namespace, svg_namespace, xml_namespace};

// ── shared lookups ──────────────────────────────────────────────────────────

/// Qualified name of a live element, else `None`.
fn qual_name(dom: &Dom, id: NodeId) -> Option<&QualName> {
    dom.element(id).map(|(name, _)| name)
}

/// Whether the element lives in the HTML namespace: the case-regime switch
/// shared with selector name matching in `select.rs`.
#[must_use]
pub fn is_html(dom: &Dom, id: NodeId) -> bool {
    qual_name(dom, id).is_some_and(|name| name.ns == html_namespace())
}

/// Whether any listed local name matches the element under its case regime:
/// ASCII-insensitive for HTML elements, exact elsewhere, so hand-built
/// `<INPUT>` behaves like tokenized `<input>`. This is the single
/// definition of that policy; selector-side name checks route here too.
#[must_use]
pub fn local_is(dom: &Dom, id: NodeId, names: &[&str]) -> bool {
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
#[must_use]
pub fn attr_value<'a>(dom: &'a Dom, id: NodeId, name: &str) -> Option<&'a str> {
    let (_, attributes) = dom.element(id)?;
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

/// First `xml:lang` in the XML namespace.
fn xml_lang_value(dom: &Dom, id: NodeId) -> Option<&str> {
    let (_, attributes) = dom.element(id)?;
    attributes.iter().find_map(|attribute| {
        (attribute.name.ns == xml_namespace() && attribute.name.local.as_ref() == "lang")
            .then_some(attribute.value.as_str())
    })
}

/// One RFC 4647 §3.3.2 *extended filtering* comparison: range subtags
/// correspond positionally to tag subtags, with ASCII-case-insensitive
/// subtag equality, `*` matches exactly one subtag, and tag subtags beyond
/// the range's length are free specificity. Compares whole subtags only (no
/// byte slicing anywhere), so arbitrary (even malformed) markup cannot panic
/// this matcher.
///
/// ```text
/// range "en"      matches "en", "en-US", "en-Latn-US"
/// range "en-Latn" matches "en-Latn", "en-Latn-US"; not "en"
/// range "*-Cyrl"  matches "sr-Cyrl"; not "sr" (`*` consumes exactly one)
/// ```
fn lang_range_matches(range: &str, tag: &str) -> bool {
    let mut tag_subtags = tag.split('-');
    for range_subtag in range.split('-') {
        match tag_subtags.next() {
            // Tag ran out first: it is less specific than the range demands.
            None => return false, // tag less specific than the range
            Some(tag_subtag) => {
                if range_subtag != "*" && !range_subtag.eq_ignore_ascii_case(tag_subtag) {
                    return false;
                }
            }
        }
    }
    true
}

// ── hyperlinks ──────────────────────────────────────────────────────────────

/// `:link` / `:any-link`, an element that is a hyperlink: local name
/// `a` or `area` carrying an `href`, in **any** namespace. SVG2 gives SVG
/// `<a>` hyperlink status the same way
/// (<https://svgwg.org/svg2-draft/linking.html#AElement>). `<link href>` is
/// not a hyperlink for matching
/// (<https://drafts.csswg.org/selectors-4/#the-any-link-pseudo>).
#[must_use]
pub fn is_hyperlink(dom: &Dom, id: NodeId) -> bool {
    local_is(dom, id, &["a", "area"]) && attr_value(dom, id, "href").is_some()
}

// ── form-control UI states ──────────────────────────────────────────────────

/// The disableable population: button, input, select, textarea, optgroup,
/// option, fieldset (<https://html.spec.whatwg.org/#concept-fe-disabled>;
/// form-associated custom elements cannot exist here). Also the population
/// [`is_enabled`] ranges over.
fn is_form_control(dom: &Dom, id: NodeId) -> bool {
    is_html(dom, id)
        && local_is(
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
/// among its ancestors, except descendants of that fieldset's first
/// `legend` child
/// (<https://html.spec.whatwg.org/#concept-fe-disabled>); or, for
/// `option`/`optgroup`, when its nearest ancestor `select` is disabled; or,
/// for an `option`, when its direct parent `optgroup` is disabled
/// (§4.10.11).
#[must_use]
pub fn is_disabled(dom: &Dom, id: NodeId) -> bool {
    if !is_form_control(dom, id) {
        return false;
    }
    if attr_value(dom, id, "disabled").is_some() {
        return true;
    }
    if local_is(dom, id, &["option", "optgroup"])
        && owning_select(dom, id)
            .is_some_and(|select| attr_value(dom, select, "disabled").is_some())
    {
        return true;
    }
    if local_is(dom, id, &["option"])
        && dom.parent(id).is_some_and(|group| {
            local_is(dom, group, &["optgroup"]) && attr_value(dom, group, "disabled").is_some()
        })
    {
        return true;
    }
    disabled_by_fieldset(dom, id)
}

fn is_descendant_of(dom: &Dom, id: NodeId, ancestor: NodeId) -> bool {
    dom.ancestors(id).any(|current| current == ancestor)
}

fn first_legend_child(dom: &Dom, fieldset: NodeId) -> Option<NodeId> {
    dom.children(fieldset)?
        .find(|&kid| local_is(dom, kid, &["legend"]))
}

fn disabled_by_fieldset(dom: &Dom, id: NodeId) -> bool {
    dom.ancestors(id).any(|ancestor| {
        local_is(dom, ancestor, &["fieldset"])
            && attr_value(dom, ancestor, "disabled").is_some()
            && first_legend_child(dom, ancestor)
                .is_none_or(|legend| !is_descendant_of(dom, id, legend))
    })
}

/// `:enabled`, the negation of [`is_disabled`] *among disableable
/// elements* (<https://html.spec.whatwg.org/#selector-enabled>): a `div`
/// without `disabled` is not "enabled", it is out of scope.
#[must_use]
pub fn is_enabled(dom: &Dom, id: NodeId) -> bool {
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
    dom.ancestors(id)
        .find(|&ancestor| local_is(dom, ancestor, &["select"]))
}

/// Whether `id` is a checkbox/radio input whose checkedness is true
/// (statically, those carrying `checked`).
fn checked_input(dom: &Dom, id: NodeId) -> bool {
    if !local_is(dom, id, &["input"]) {
        return false;
    }
    let ty = attr_value(dom, id, "type").unwrap_or("text");
    (ty.eq_ignore_ascii_case("checkbox") || ty.eq_ignore_ascii_case("radio"))
        && attr_value(dom, id, "checked").is_some()
}

/// `:checked` per HTML §4.16.3 (<https://html.spec.whatwg.org/#selector-checked>):
/// checkbox/radio inputs whose checkedness is true (statically, those
/// carrying `checked`), or options whose selectedness is true. Selectedness
/// has a static default (in a select without `multiple`, the first option
/// of its list of options is selected when nothing in that list carries
/// `selected`, and the list flattens `optgroup`s), so fresh parsed pages
/// answer as browsers do.
#[must_use]
pub fn is_checked(dom: &Dom, id: NodeId) -> bool {
    if checked_input(dom, id) {
        return true;
    }
    if local_is(dom, id, &["option"]) {
        if has_selected_attribute(dom, id) {
            return true;
        }
        let Some(select) = owning_select(dom, id) else {
            return false;
        };
        if attr_value(dom, select, "multiple").is_some() {
            return false;
        }
        // HTML's "list of options", within which `optgroup`s (and anything
        // else wrapping them) are transparent containers.
        let options: Vec<NodeId> = dom
            .descendants(select)
            .filter(|&option| local_is(dom, option, &["option"]))
            .collect();
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
    is_html(dom, id) && local_is(dom, id, &["input", "select", "textarea"])
}

/// `:required` (<https://html.spec.whatwg.org/#selector-required>).
#[must_use]
pub fn is_required(dom: &Dom, id: NodeId) -> bool {
    constraint_target(dom, id) && attr_value(dom, id, "required").is_some()
}

/// `:optional`: the same population without `required`.
#[must_use]
pub fn is_optional(dom: &Dom, id: NodeId) -> bool {
    constraint_target(dom, id) && attr_value(dom, id, "required").is_none()
}

/// Input types to which the `readonly` attribute does **not** apply
/// (<https://html.spec.whatwg.org/multipage/input.html#attr-input-readonly>).
/// An invalid `type` takes the Text state, where `readonly` applies, so this
/// is an exclusion set rather than an allowlist.
fn readonly_applies(dom: &Dom, id: NodeId) -> bool {
    let ty = attr_value(dom, id, "type").unwrap_or("text");
    ![
        "hidden", "checkbox", "radio", "file", "submit", "image", "reset", "button", "color",
        "range",
    ]
    .iter()
    .any(|excluded| ty.eq_ignore_ascii_case(excluded))
}

/// `:read-write`, an editable control
/// (<https://drafts.csswg.org/selectors-4/#read-write-pseudo>): an `input`
/// to which `readonly` applies that is neither `readonly` nor disabled, or a
/// `textarea` that is neither `readonly` nor disabled. Editing hosts
/// (`contenteditable`) have no representation in this tree yet.
#[must_use]
pub fn is_read_write(dom: &Dom, id: NodeId) -> bool {
    if !is_html(dom, id) {
        return false;
    }
    if local_is(dom, id, &["textarea"]) {
        return attr_value(dom, id, "readonly").is_none() && !is_disabled(dom, id);
    }
    if local_is(dom, id, &["input"]) {
        return readonly_applies(dom, id)
            && attr_value(dom, id, "readonly").is_none()
            && !is_disabled(dom, id);
    }
    false
}

/// `:read-only`: every element that is not `:read-write`
/// (<https://drafts.csswg.org/selectors-4/#read-only-pseudo>). This is the
/// spec's complement, not a form-control population: a `div` is read-only.
#[must_use]
pub fn is_read_only(dom: &Dom, id: NodeId) -> bool {
    !is_read_write(dom, id)
}

/// Input types that can present a placeholder
/// (<https://html.spec.whatwg.org/#attr-input-placeholder>: textual and
/// numeric-entry types only; a checkbox shows nothing). An invalid `type`
/// takes the Text state, so this excludes the types that never show one.
fn placeholder_capable_type(dom: &Dom, id: NodeId) -> bool {
    let ty = attr_value(dom, id, "type").unwrap_or("text");
    ![
        "hidden",
        "checkbox",
        "radio",
        "file",
        "submit",
        "image",
        "reset",
        "button",
        "color",
        "range",
        "date",
        "month",
        "week",
        "time",
        "datetime-local",
    ]
    .iter()
    .any(|excluded| ty.eq_ignore_ascii_case(excluded))
}

/// `:placeholder-shown`: a placeholder is *shown* only while the control's
/// value is empty (<https://html.spec.whatwg.org/#attr-input-placeholder>).
/// The value is the *live* value, not the content attribute: a dirty value
/// set through the IDL hides the placeholder. For an `input` that means a
/// placeholder-capable type whose [`Dom::input_value`] is empty; for a
/// `textarea` the value is its text content.
#[must_use]
pub fn is_placeholder_shown(dom: &Dom, id: NodeId) -> bool {
    if !is_html(dom, id) || attr_value(dom, id, "placeholder").is_none() {
        return false;
    }
    if local_is(dom, id, &["input"]) {
        if !placeholder_capable_type(dom, id) {
            return false;
        }
        return dom.input_value(id).is_none_or(|value| value.is_empty());
    }
    if local_is(dom, id, &["textarea"]) {
        return dom
            .textarea_value(id)
            .is_none_or(|value| value.is_empty());
    }
    false
}

/// `:default`, static subset of <https://html.spec.whatwg.org/#selector-default>:
/// checkbox/radio inputs with `checked`, options with `selected`. Form
/// default-submit buttons are not represented (no form-owner association).
#[must_use]
pub fn is_default(dom: &Dom, id: NodeId) -> bool {
    checked_input(dom, id) || (local_is(dom, id, &["option"]) && has_selected_attribute(dom, id))
}

/// `:indeterminate`, static subset: a `progress` without a `value`
/// attribute (<https://html.spec.whatwg.org/#the-progress-element>). Radio
/// groups are not represented (no form-owner association).
#[must_use]
pub fn is_indeterminate(dom: &Dom, id: NodeId) -> bool {
    if local_is(dom, id, &["progress"]) {
        return attr_value(dom, id, "value").is_none();
    }
    if local_is(dom, id, &["input"]) {
        let typ = attr_value(dom, id, "type").unwrap_or_default().to_ascii_lowercase();
        if typ == "checkbox" {
            return dom.indeterminate(id);
        }
        if typ == "radio" {
            return dom.radio_group_checked(id).is_none();
        }
    }
    false
}

/// `:defined` per <https://html.spec.whatwg.org/#selector-defined>: an
/// element is undefined when it is a valid-but-unregistered *custom
/// element*, an HTML-ns name containing `-` that is not one of the
/// reserved hyphenated names. No custom-element registry exists here, so
/// every other hyphenated HTML name is undefined.
#[must_use]
pub fn is_defined(dom: &Dom, id: NodeId) -> bool {
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
        return true;
    }
    let local = name.local.as_ref();
    if !local.contains('-') {
        return true;
    }
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
/// `lang_range_matches` for the exact algorithm, including wildcards.
///
/// `xml:lang` takes precedence over `lang`, then the document
/// `Content-Language` default
/// (<https://html.spec.whatwg.org/multipage/dom.html#language>).
#[must_use]
pub fn lang_matches(dom: &Dom, id: NodeId, ranges: &[Box<str>]) -> bool {
    let found = std::iter::once(id)
        .chain(dom.ancestors(id))
        .find_map(|current| {
            xml_lang_value(dom, current).or_else(|| {
                let (name, _) = dom.element(current)?;
                (name.ns == html_namespace() || name.ns == svg_namespace())
                    .then(|| attr_value(dom, current, "lang"))
                    .flatten()
            })
        });
    let tag = found.or_else(|| dom.document_language());
    let Some(tag) = tag else {
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
        .filter(|value| value.eq_ignore_ascii_case("ltr") || value.eq_ignore_ascii_case("rtl"))
}

/// `:dir(direction)`: nearest HTML ancestor-or-self with a `dir` attribute
/// of `ltr`/`rtl` (<https://drafts.csswg.org/selectors-4/#dir-pseudo>).
/// Defaults to `ltr`. `dir="auto"` is not classified (needs first-strong
/// bidi); invalid values inherit, per Undefined direction.
#[must_use]
pub fn direction_is(dom: &Dom, id: NodeId, want: &str) -> bool {
    std::iter::once(id)
        .chain(dom.ancestors(id))
        .find_map(|current| dir_attr(dom, current))
        .map_or_else(
            || want.eq_ignore_ascii_case("ltr"),
            |found| found.eq_ignore_ascii_case(want),
        )
}
