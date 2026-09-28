//! The `input` and `textarea` value model: the per-state value modes, the
//! value sanitization algorithms, the dirty value flag, the type-change steps,
//! and text-control selection
//! (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).

use crate::{
    Document, DomError, NodeId, html_namespace, is_valid_date, is_valid_floating_point,
    is_valid_local_date_time, is_valid_month, is_valid_simple_color, is_valid_time, is_valid_week,
};

const INPUT_TYPES: &[&str] = &[
    "hidden",
    "text",
    "search",
    "tel",
    "url",
    "email",
    "password",
    "date",
    "month",
    "week",
    "time",
    "datetime-local",
    "number",
    "range",
    "color",
    "checkbox",
    "radio",
    "file",
    "submit",
    "image",
    "reset",
    "button",
];

    /// The live value of an HTML `input` element.
    ///
    /// Before the dirty value flag is set, the value follows the content
    /// attribute; setting the IDL value stores an independent value
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    #[must_use]
    pub fn input_value(document: &Document, id: NodeId) -> Option<String> {
        let typ = input_type(document, id)?;
        Some(match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = document
                    .form
                    .input_values
                    .get(&id)
                    .cloned()
                    .or_else(|| document.attribute(id, "value"))
                    .unwrap_or_default();
                sanitize_input_value(document, id, value)
            }
            ValueMode::Default => document.attribute(id, "value").unwrap_or_default(),
            ValueMode::DefaultOn => document
                .attribute(id, "value")
                .unwrap_or_else(|| "on".to_owned()),
            ValueMode::Filename => String::new(),
        })
    }

    /// Sets an HTML `input` element's live value and its dirty value flag.
    ///
    /// <https://html.spec.whatwg.org/multipage/input.html#dom-input-value>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input`,
    /// or [`DomError::InvalidState`] when setting a non-empty value on a
    /// `type=file` input.
    pub fn set_input_value(document: &mut Document, id: NodeId, value: String) -> Result<(), DomError> {
        let typ = input_type(document, id).ok_or(DomError::WrongNodeType)?;
        match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = sanitize_input_value(document, id, value);
                document.form.input_values.insert(id, value);
            }
            ValueMode::Default | ValueMode::DefaultOn => {
                crate::mutation::set_attribute(document, id, "value", value)?;
            }
            ValueMode::Filename => {
                if !value.is_empty() {
                    return Err(DomError::InvalidState);
                }
            }
        }
        Ok(())
    }

    /// Runs the input type-change steps after the `type` attribute changed,
    /// comparing against the last normalized state
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
    pub(crate) fn refresh_input_type(document: &mut Document, id: NodeId) -> Result<(), DomError> {
        let Some(new_type) = input_type(document, id) else {
            return Ok(());
        };
        let old = document
            .form
            .input_types
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "text".to_owned());
        document.form.input_types.insert(id, new_type.clone());
        if old == new_type {
            return Ok(());
        }
        apply_input_type_migration(document, id, &old, &new_type)
    }

    /// Migrates an input's value between value modes and re-sanitizes it for
    /// the new state
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
    fn apply_input_type_migration(
        document: &mut Document,
        id: NodeId,
        old: &str,
        new_type: &str,
    ) -> Result<(), DomError> {
        match (input_value_mode(old), input_value_mode(new_type)) {
            // A value-mode value becomes the new default, unless it is empty.
            (ValueMode::Value, ValueMode::Default | ValueMode::DefaultOn) => {
                let raw = document
                    .form
                    .input_values
                    .get(&id)
                    .cloned()
                    .or_else(|| document.attribute(id, "value"))
                    .unwrap_or_default();
                let value = sanitize_value_for_type(document, id, old, raw);
                if !value.is_empty() {
                    crate::mutation::set_attribute(document, id, "value", value)?;
                }
                document.form.input_values.remove(&id);
            }
            // A non-value state follows the content attribute again.
            (mode, ValueMode::Value) if mode != ValueMode::Value => {
                document.form.input_values.remove(&id);
            }
            // A non-filename state clears the value for a file input.
            (mode, ValueMode::Filename) if mode != ValueMode::Filename => {
                document.form.input_values.remove(&id);
            }
            _ => {}
        }
        // The type-change steps invoke the new state's value sanitization.
        if input_value_mode(new_type) == ValueMode::Value
            && let Some(value) = document.form.input_values.get(&id).cloned()
        {
            let sanitized = sanitize_value_for_type(document, id, new_type, value);
            document.form.input_values.insert(id, sanitized);
        }
        Ok(())
    }

    /// The normalized type state of an HTML `input` element.
    #[must_use]
    pub fn input_type(document: &Document, id: NodeId) -> Option<String> {
        input_value_element(document, id)?;
        let value = document
            .attribute(id, "type")
            .unwrap_or_else(|| "text".into())
            .to_ascii_lowercase();
        Some(if INPUT_TYPES.contains(&value.as_str()) {
            value
        } else {
            "text".into()
        })
    }

    fn input_value_element(document: &Document, id: NodeId) -> Option<()> {
        let (name, _) = document.element(id)?;
        (name.ns == html_namespace() && name.local.as_ref() == "input").then_some(())
    }

    /// Applies the value sanitization algorithm for each input state whose
    /// value has a grammar. A string that does not match is replaced by the
    /// state's default (the empty string, or `#000000` for color)
    /// <https://html.spec.whatwg.org/multipage/input.html#value-sanitization-algorithm>.
    fn sanitize_input_value(document: &Document, id: NodeId, value: String) -> String {
        let typ = input_type(document, id).unwrap_or_else(|| "text".into());
        sanitize_value_for_type(document, id, &typ, value)
    }

    /// The value sanitization algorithm for a named state; used by
    /// [`Self::sanitize_input_value`] and by the type-change steps, which must
    /// sanitize for the new state before the `type` attribute lands.
    fn sanitize_value_for_type(document: &Document, id: NodeId, typ: &str, value: String) -> String {
        match typ {
            "text" | "search" | "tel" | "password" => value.replace(['\r', '\n'], ""),
            "url" | "email" => value
                .replace(['\r', '\n'], "")
                .trim_matches(|character: char| character.is_ascii_whitespace())
                .to_owned(),
            "number" => sanitize_grammar(value, is_valid_floating_point),
            "range" => sanitize_range_value(
                &value,
                document.attribute(id, "min").as_deref().and_then(parse_finite),
                document.attribute(id, "max").as_deref().and_then(parse_finite),
                document.attribute(id, "value")
                    .as_deref()
                    .and_then(parse_finite),
                document.attribute(id, "step").as_deref(),
            ),
            "date" => sanitize_grammar(value, is_valid_date),
            "month" => sanitize_grammar(value, is_valid_month),
            "week" => sanitize_grammar(value, is_valid_week),
            "time" => sanitize_grammar(value, is_valid_time),
            "datetime-local" => sanitize_local_date_time_value(&value),
            "color" => {
                if is_valid_simple_color(&value) {
                    value.to_ascii_lowercase()
                } else {
                    "#000000".to_owned()
                }
            }
            _ => value,
        }
    }

    /// The raw value of an HTML `textarea`: its stored raw value while the
    /// dirty value flag is set, otherwise its child text content.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#concept-textarea-raw-value>
    #[must_use]
    pub fn textarea_raw_value(document: &Document, id: NodeId) -> Option<String> {
        if !document.html_local_is(id, "textarea") {
            return None;
        }
        Some(
            document.form
                .input_values
                .get(&id)
                .cloned()
                .unwrap_or_else(|| document.child_text_content(id)),
        )
    }

    /// The API value of an HTML `textarea`: its raw value with newlines
    /// normalized to LF.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#concept-fe-api-value>
    #[must_use]
    pub fn textarea_value(document: &Document, id: NodeId) -> Option<String> {
        textarea_raw_value(document, id)
            .map(|value| normalize_newlines(&value))
    }

    /// Sets an HTML `textarea`'s raw value and dirty value flag.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `textarea`.
    pub fn set_textarea_value(document: &mut Document, id: NodeId, value: String) -> Result<(), DomError> {
        if !document.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        document.form.input_values.insert(id, value);
        Ok(())
    }

    /// The live value of a text-like control, as the `value` IDL attribute
    /// reports it: the API value for `input` and `textarea`.
    #[must_use]
    pub fn control_value(document: &Document, id: NodeId) -> Option<String> {
        if document.html_local_is(id, "input") {
            input_value(document, id)
        } else {
            textarea_value(document, id)
        }
    }

    /// Sets the live value of a text-like control and its dirty value flag.
    ///
    /// When the API value changes, the text entry cursor moves to the end and
    /// the selection direction resets
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>,
    /// <https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input` or
    /// `textarea`.
    pub fn set_control_value(document: &mut Document, id: NodeId, value: String) -> Result<(), DomError> {
        let old = control_value(document, id);
        if document.html_local_is(id, "input") {
            set_input_value(document, id, value)?;
        } else {
            set_textarea_value(document, id, value)?;
        }
        if selection_supported(document, id) && control_value(document, id) != old {
            let end = control_value(document, id)
                .map_or(0, |value| utf16_length(&value));
            set_selection(document, id, end, end, 0);
        }
        Ok(())
    }

    /// Whether a text selection applies to `id`: an `HTMLTextAreaElement`, or
    /// an `HTMLInputElement` whose type is a text-entry type. Other input
    /// types report null and rejecting setters
    /// (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
    #[must_use]
    pub fn selection_supported(document: &Document, id: NodeId) -> bool {
        if document.html_local_is(id, "textarea") {
            return true;
        }
        if !document.html_local_is(id, "input") {
            return false;
        }
        matches!(
            input_type(document, id).as_deref(),
            Some("text" | "search" | "tel" | "url" | "password")
        )
    }

    /// The stored selection `(start, end, direction)`, clamped to the current
    /// API value length. `None` when no text selection applies.
    #[must_use]
    pub fn selection(document: &Document, id: NodeId) -> Option<(u32, u32, u8)> {
        if !selection_supported(document, id) {
            return None;
        }
        let length = control_value(document, id)
            .map_or(0, |value| utf16_length(&value));
        let (start, end, direction) = document.form.selections.get(&id).copied().unwrap_or((0, 0, 0));
        Some((start.min(length), end.min(length), direction))
    }

    /// The API value of a non-dirty `textarea` `parent`, or `None` when
    /// `parent` is not such a control. Used to detect whether a child change
    /// really changed the value.
    pub(crate) fn textarea_value_before_change(document: &Document, parent: NodeId) -> Option<String> {
        if !document.html_local_is(parent, "textarea") || document.form.input_values.contains_key(&parent) {
            return None;
        }
        textarea_value(document, parent)
    }

    /// A `textarea` with no stored raw value (dirty value flag unset) derives
    /// its value from its children, so a child change that alters the API value
    /// invalidates its selection: reset it to the start. A change that leaves
    /// the API value alone (for example one that only differs in raw newlines)
    /// keeps the selection. A dirty textarea keeps both its value and its
    /// selection.
    pub(crate) fn reset_textarea_selection_if_changed(
        document: &mut Document,
        parent: NodeId,
        before: Option<String>,
    ) {
        let Some(before) = before else {
            return;
        };
        if textarea_value(document, parent).as_deref() != Some(before.as_str()) {
            document.form.selections.insert(parent, (0, 0, 0));
        }
    }

    /// Stores `id`'s selection, clamped to the current API value length, and
    /// reports whether the stored tuple actually changed (so the caller can
    /// queue a `select` event only for a real modification).
    pub fn set_selection(document: &mut Document, id: NodeId, start: u32, end: u32, direction: u8) -> bool {
        if !selection_supported(document, id) {
            return false;
        }
        let length = control_value(document, id)
            .map_or(0, |value| utf16_length(&value));
        let mut end = end.min(length);
        let mut start = start.min(length);
        // If end is less than or equal to start, both are placed immediately
        // before the character with offset end
        // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#set-the-selection-range>).
        if end <= start {
            start = end;
        }
        end = end.max(start);
        let next = (start, end, direction.min(2));
        let previous = document.form.selections.get(&id).copied().unwrap_or((0, 0, 0));
        document.form.selections.insert(id, next);
        next != previous
    }

    /// Whether `id`'s input type supported a text selection when its `type`
    /// last changed. The input default (`type=text`) is selectable.
    #[must_use]
    pub fn input_selectable(document: &Document, id: NodeId) -> bool {
        document.form.input_selectable.get(&id).copied().unwrap_or(true)
    }

    /// Records whether `id`'s input type currently supports a text selection.
    pub fn set_input_selectable(document: &mut Document, id: NodeId, selectable: bool) {
        document.form.input_selectable.insert(id, selectable);
    }

    /// The `value` IDL value for any element that has one: `option`, `select`,
    /// or a text-like control.
    #[must_use]
    pub fn element_value(document: &Document, id: NodeId) -> Option<String> {
        if document.html_local_is(id, "option") {
            Some(super::select::option_value(document, id))
        } else if document.html_local_is(id, "select") {
            Some(super::select::select_value(document, id))
        } else {
            control_value(document, id)
        }
    }

    /// Sets the `value` IDL value for `option`, `select`, or a text-like
    /// control.
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` has no `value`.
    pub fn set_element_value(document: &mut Document, id: NodeId, value: String) -> Result<(), DomError> {
        if document.html_local_is(id, "option") {
            return crate::mutation::set_attribute(document, id, "value", value);
        }
        if document.html_local_is(id, "select") {
            super::select::set_select_value(document, id, &value);
            return Ok(());
        }
        set_control_value(document, id, value)
    }

    /// The `defaultValue` of a text-like control: the `value` content
    /// attribute for `input`, the child text content for `textarea`.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue>
    #[must_use]
    pub fn control_default_value(document: &Document, id: NodeId) -> Option<String> {
        if document.html_local_is(id, "input") {
            Some(document.attribute(id, "value").unwrap_or_default())
        } else if document.html_local_is(id, "textarea") {
            Some(document.child_text_content(id))
        } else {
            None
        }
    }

    /// Sets the `defaultValue` of a text-like control: assigning the `value`
    /// content attribute for `input`, string-replacing all children for
    /// `textarea`.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `input` or
    /// `textarea`.
    pub fn set_control_default_value(document: &mut Document, id: NodeId, value: String) -> Result<(), DomError> {
        if document.html_local_is(id, "input") {
            return crate::mutation::set_attribute(document, id, "value", value);
        }
        if !document.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        let replacement = document.create_fragment();
        if !value.is_empty() {
            let text = document.create_text(value);
            crate::mutation::append(document, replacement, text)?;
        }
        crate::mutation::replace_all(document, id, replacement)
    }

/// Keeps `value` when it satisfies `valid`, else replaces it with the empty
/// string, the default for every grammar-constrained input state except color.
fn sanitize_grammar(value: String, valid: fn(&str) -> bool) -> String {
    if valid(&value) { value } else { String::new() }
}
/// The `value` IDL attribute's mode for an input state
/// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:value-mode>).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueMode {
    Value,
    Default,
    DefaultOn,
    Filename,
}
fn input_value_mode(typ: &str) -> ValueMode {
    match typ {
        "checkbox" | "radio" => ValueMode::DefaultOn,
        "file" => ValueMode::Filename,
        "hidden" | "submit" | "image" | "reset" | "button" => ValueMode::Default,
        _ => ValueMode::Value,
    }
}
/// Parses a finite floating-point number, or `None` when the text is not a
/// valid floating-point number.
fn parse_finite(text: &str) -> Option<f64> {
    if !is_valid_floating_point(text) {
        return None;
    }
    text.parse::<f64>().ok().filter(|number| number.is_finite())
}
/// The local date and time state's value sanitization: the separator becomes
/// `T` and the time is normalized.
fn sanitize_local_date_time_value(value: &str) -> String {
    if !is_valid_local_date_time(value) {
        return String::new();
    }
    let Some((date, time)) = value.split_once(['T', ' ']) else {
        return String::new();
    };
    format!("{date}T{}", normalize_time(time))
}
/// Drops a zero seconds field and trailing zeros from the fraction of a valid
/// time string, so it is the shortest form
/// (<https://html.spec.whatwg.org/multipage/input.html#time-state-(type=time)>).
fn normalize_time(time: &str) -> String {
    let mut result = time.to_owned();
    if let Some((base, fraction)) = result.split_once('.') {
        let trimmed = fraction.trim_end_matches('0');
        if trimmed.is_empty() {
            result.truncate(base.len());
        } else if trimmed.len() != fraction.len() {
            result.truncate(base.len() + 1 + trimmed.len());
        }
    }
    let parts: Vec<&str> = result.split(':').collect();
    if parts.len() == 3 && parts[2] == "00" {
        result = format!("{}:{}", parts[0], parts[1]);
    }
    result
}
/// The range state's value sanitization: an invalid value becomes the default
/// value, the value is clamped to the range, and a step mismatch rounds to the
/// nearest representable value in the range
/// (<https://html.spec.whatwg.org/multipage/input.html#range-state-(type=range)>).
fn sanitize_range_value(
    value: &str,
    min_attr: Option<f64>,
    max_attr: Option<f64>,
    value_attr: Option<f64>,
    step_attr: Option<&str>,
) -> String {
    // The range state defines a default minimum of 0 and a default maximum of
    // 100.
    let min = min_attr.unwrap_or(0.0);
    let max = max_attr.unwrap_or(100.0);
    // The default value is the midpoint, or the minimum when the range is
    // reversed.
    let mut number = parse_finite(value).unwrap_or_else(|| {
        if max < min {
            min
        } else {
            min + (max - min) / 2.0
        }
    });
    // Underflow, then overflow (only when the maximum is not less than the
    // minimum).
    number = number.max(min);
    if max >= min {
        number = number.min(max);
    }
    // A step mismatch rounds to the nearest representable value within the
    // range, ties toward positive infinity.
    let step_any = step_attr.is_some_and(|text| text.trim().eq_ignore_ascii_case("any"));
    if !step_any {
        let step = step_attr
            .and_then(parse_finite)
            .filter(|step| *step > 0.0)
            .unwrap_or(1.0);
        // The step base is the min attribute, else the value attribute, else 0.
        let base = min_attr.or(value_attr).unwrap_or(0.0);
        let quotient = (number - base) / step;
        if (quotient - quotient.round()).abs() > 1e-9 {
            let in_range =
                |candidate: f64| candidate >= min - 1e-9 && (max < min || candidate <= max + 1e-9);
            let mut best: Option<f64> = None;
            for candidate in [
                base + quotient.floor() * step,
                base + quotient.ceil() * step,
            ] {
                if !in_range(candidate) {
                    continue;
                }
                let keep = match best {
                    None => true,
                    Some(current) => {
                        let current_distance = (current - number).abs();
                        let candidate_distance = (candidate - number).abs();
                        candidate_distance < current_distance - 1e-9
                            || ((candidate_distance - current_distance).abs() <= 1e-9
                                && candidate > current)
                    }
                };
                if keep {
                    best = Some(candidate);
                }
            }
            if let Some(candidate) = best {
                number = candidate;
            }
        }
    }
    format!("{number}")
}
/// Replaces CRLF and lone CR with LF
/// (<https://infra.spec.whatwg.org/#normalize-newlines>).
fn normalize_newlines(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    normalized
}
/// The [length](https://infra.spec.whatwg.org/#string-length) of `text` in
/// UTF-16 code units, the unit the selection APIs use.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a 64-bit string length beyond u32 cannot be produced by this engine"
)]
fn utf16_length(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}
