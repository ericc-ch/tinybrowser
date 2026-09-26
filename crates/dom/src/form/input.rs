//! The `input` and `textarea` value model: the per-state value modes, the
//! value sanitization algorithms, the dirty value flag, the type-change steps,
//! and text-control selection
//! (<https://html.spec.whatwg.org/multipage/input.html#the-input-element>).

use crate::{
    Dom, DomError, NodeId, html_namespace, is_valid_date, is_valid_floating_point,
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

impl Dom {
    /// The live value of an HTML `input` element.
    ///
    /// Before the dirty value flag is set, the value follows the content
    /// attribute; setting the IDL value stores an independent value
    /// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
    #[must_use]
    pub fn input_value(&self, id: NodeId) -> Option<String> {
        let typ = self.input_type(id)?;
        Some(match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = self
                    .input_values
                    .get(&id)
                    .cloned()
                    .or_else(|| self.attribute(id, "value"))
                    .unwrap_or_default();
                self.sanitize_input_value(id, value)
            }
            ValueMode::Default => self.attribute(id, "value").unwrap_or_default(),
            ValueMode::DefaultOn => self
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
    pub fn set_input_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        let typ = self.input_type(id).ok_or(DomError::WrongNodeType)?;
        match input_value_mode(&typ) {
            ValueMode::Value => {
                let value = self.sanitize_input_value(id, value);
                self.input_values.insert(id, value);
            }
            ValueMode::Default | ValueMode::DefaultOn => {
                self.set_attribute(id, "value", value)?;
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
    pub(crate) fn refresh_input_type(&mut self, id: NodeId) -> Result<(), DomError> {
        let Some(new_type) = self.input_type(id) else {
            return Ok(());
        };
        let old = self
            .input_types
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "text".to_owned());
        self.input_types.insert(id, new_type.clone());
        if old == new_type {
            return Ok(());
        }
        self.apply_input_type_migration(id, &old, &new_type)
    }

    /// Migrates an input's value between value modes and re-sanitizes it for
    /// the new state
    /// (<https://html.spec.whatwg.org/multipage/input.html#the-input-element:type-change-state>).
    fn apply_input_type_migration(
        &mut self,
        id: NodeId,
        old: &str,
        new_type: &str,
    ) -> Result<(), DomError> {
        match (input_value_mode(old), input_value_mode(new_type)) {
            // A value-mode value becomes the new default, unless it is empty.
            (ValueMode::Value, ValueMode::Default | ValueMode::DefaultOn) => {
                let raw = self
                    .input_values
                    .get(&id)
                    .cloned()
                    .or_else(|| self.attribute(id, "value"))
                    .unwrap_or_default();
                let value = self.sanitize_value_for_type(id, old, raw);
                if !value.is_empty() {
                    self.set_attribute(id, "value", value)?;
                }
                self.input_values.remove(&id);
            }
            // A non-value state follows the content attribute again.
            (mode, ValueMode::Value) if mode != ValueMode::Value => {
                self.input_values.remove(&id);
            }
            // A non-filename state clears the value for a file input.
            (mode, ValueMode::Filename) if mode != ValueMode::Filename => {
                self.input_values.remove(&id);
            }
            _ => {}
        }
        // The type-change steps invoke the new state's value sanitization.
        if input_value_mode(new_type) == ValueMode::Value
            && let Some(value) = self.input_values.get(&id).cloned()
        {
            let sanitized = self.sanitize_value_for_type(id, new_type, value);
            self.input_values.insert(id, sanitized);
        }
        Ok(())
    }

    /// The normalized type state of an HTML `input` element.
    #[must_use]
    pub fn input_type(&self, id: NodeId) -> Option<String> {
        self.input_value_element(id)?;
        let value = self
            .attribute(id, "type")
            .unwrap_or_else(|| "text".into())
            .to_ascii_lowercase();
        Some(if INPUT_TYPES.contains(&value.as_str()) {
            value
        } else {
            "text".into()
        })
    }

    fn input_value_element(&self, id: NodeId) -> Option<()> {
        let (name, _) = self.element(id)?;
        (name.ns == html_namespace() && name.local.as_ref() == "input").then_some(())
    }

    /// Applies the value sanitization algorithm for each input state whose
    /// value has a grammar. A string that does not match is replaced by the
    /// state's default (the empty string, or `#000000` for color)
    /// <https://html.spec.whatwg.org/multipage/input.html#value-sanitization-algorithm>.
    fn sanitize_input_value(&self, id: NodeId, value: String) -> String {
        let typ = self.input_type(id).unwrap_or_else(|| "text".into());
        self.sanitize_value_for_type(id, &typ, value)
    }

    /// The value sanitization algorithm for a named state; used by
    /// [`Self::sanitize_input_value`] and by the type-change steps, which must
    /// sanitize for the new state before the `type` attribute lands.
    fn sanitize_value_for_type(&self, id: NodeId, typ: &str, value: String) -> String {
        match typ {
            "text" | "search" | "tel" | "password" => value.replace(['\r', '\n'], ""),
            "url" | "email" => value
                .replace(['\r', '\n'], "")
                .trim_matches(|character: char| character.is_ascii_whitespace())
                .to_owned(),
            "number" => sanitize_grammar(value, is_valid_floating_point),
            "range" => sanitize_range_value(
                &value,
                self.attribute(id, "min").as_deref().and_then(parse_finite),
                self.attribute(id, "max").as_deref().and_then(parse_finite),
                self.attribute(id, "value").as_deref().and_then(parse_finite),
                self.attribute(id, "step").as_deref(),
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
    pub fn textarea_raw_value(&self, id: NodeId) -> Option<String> {
        if !self.html_local_is(id, "textarea") {
            return None;
        }
        Some(
            self.input_values
                .get(&id)
                .cloned()
                .unwrap_or_else(|| self.child_text_content(id)),
        )
    }

    /// The API value of an HTML `textarea`: its raw value with newlines
    /// normalized to LF.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#concept-fe-api-value>
    #[must_use]
    pub fn textarea_value(&self, id: NodeId) -> Option<String> {
        self.textarea_raw_value(id)
            .map(|value| normalize_newlines(&value))
    }

    /// Sets an HTML `textarea`'s raw value and dirty value flag.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` is not an HTML `textarea`.
    pub fn set_textarea_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if !self.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        self.input_values.insert(id, value);
        Ok(())
    }

    /// The live value of a text-like control, as the `value` IDL attribute
    /// reports it: the API value for `input` and `textarea`.
    #[must_use]
    pub fn control_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "input") {
            self.input_value(id)
        } else {
            self.textarea_value(id)
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
    pub fn set_control_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        let old = self.control_value(id);
        if self.html_local_is(id, "input") {
            self.set_input_value(id, value)?;
        } else {
            self.set_textarea_value(id, value)?;
        }
        if self.selection_supported(id) && self.control_value(id) != old {
            let end = self
                .control_value(id)
                .map_or(0, |value| utf16_length(&value));
            self.set_selection(id, end, end, 0);
        }
        Ok(())
    }

    /// Whether a text selection applies to `id`: an `HTMLTextAreaElement`, or
    /// an `HTMLInputElement` whose type is a text-entry type. Other input
    /// types report null and rejecting setters
    /// (<https://html.spec.whatwg.org/multipage/input.html#do-not-apply>).
    #[must_use]
    pub fn selection_supported(&self, id: NodeId) -> bool {
        if self.html_local_is(id, "textarea") {
            return true;
        }
        if !self.html_local_is(id, "input") {
            return false;
        }
        matches!(
            self.input_type(id).as_deref(),
            Some("text" | "search" | "tel" | "url" | "password")
        )
    }

    /// The stored selection `(start, end, direction)`, clamped to the current
    /// API value length. `None` when no text selection applies.
    #[must_use]
    pub fn selection(&self, id: NodeId) -> Option<(u32, u32, u8)> {
        if !self.selection_supported(id) {
            return None;
        }
        let length = self.control_value(id).map_or(0, |value| utf16_length(&value));
        let (start, end, direction) = self.selections.get(&id).copied().unwrap_or((0, 0, 0));
        Some((start.min(length), end.min(length), direction))
    }

    /// The API value of a non-dirty `textarea` `parent`, or `None` when
    /// `parent` is not such a control. Used to detect whether a child change
    /// really changed the value.
    pub(crate) fn textarea_value_before_change(&self, parent: NodeId) -> Option<String> {
        if !self.html_local_is(parent, "textarea") || self.input_values.contains_key(&parent) {
            return None;
        }
        self.textarea_value(parent)
    }

    /// A `textarea` with no stored raw value (dirty value flag unset) derives
    /// its value from its children, so a child change that alters the API value
    /// invalidates its selection: reset it to the start. A change that leaves
    /// the API value alone (for example one that only differs in raw newlines)
    /// keeps the selection. A dirty textarea keeps both its value and its
    /// selection.
    pub(crate) fn reset_textarea_selection_if_changed(&mut self, parent: NodeId, before: Option<String>) {
        let Some(before) = before else {
            return;
        };
        if self.textarea_value(parent).as_deref() != Some(before.as_str()) {
            self.selections.insert(parent, (0, 0, 0));
        }
    }

    /// Stores `id`'s selection, clamped to the current API value length, and
    /// reports whether the stored tuple actually changed (so the caller can
    /// queue a `select` event only for a real modification).
    pub fn set_selection(&mut self, id: NodeId, start: u32, end: u32, direction: u8) -> bool {
        if !self.selection_supported(id) {
            return false;
        }
        let length = self.control_value(id).map_or(0, |value| utf16_length(&value));
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
        let previous = self.selections.get(&id).copied().unwrap_or((0, 0, 0));
        self.selections.insert(id, next);
        next != previous
    }

    /// Whether `id`'s input type supported a text selection when its `type`
    /// last changed. The input default (`type=text`) is selectable.
    #[must_use]
    pub fn input_selectable(&self, id: NodeId) -> bool {
        self.input_selectable.get(&id).copied().unwrap_or(true)
    }

    /// Records whether `id`'s input type currently supports a text selection.
    pub fn set_input_selectable(&mut self, id: NodeId, selectable: bool) {
        self.input_selectable.insert(id, selectable);
    }

    /// The `value` IDL value for any element that has one: `option`, `select`,
    /// or a text-like control.
    #[must_use]
    pub fn element_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "option") {
            Some(self.option_value(id))
        } else if self.html_local_is(id, "select") {
            Some(self.select_value(id))
        } else {
            self.control_value(id)
        }
    }

    /// Sets the `value` IDL value for `option`, `select`, or a text-like
    /// control.
    ///
    /// # Errors
    ///
    /// Returns [`DomError::WrongNodeType`] when `id` has no `value`.
    pub fn set_element_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if self.html_local_is(id, "option") {
            return self.set_attribute(id, "value", value);
        }
        if self.html_local_is(id, "select") {
            self.set_select_value(id, &value);
            return Ok(());
        }
        self.set_control_value(id, value)
    }

    /// The `defaultValue` of a text-like control: the `value` content
    /// attribute for `input`, the child text content for `textarea`.
    ///
    /// <https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue>
    #[must_use]
    pub fn control_default_value(&self, id: NodeId) -> Option<String> {
        if self.html_local_is(id, "input") {
            Some(self.attribute(id, "value").unwrap_or_default())
        } else if self.html_local_is(id, "textarea") {
            Some(self.child_text_content(id))
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
    pub fn set_control_default_value(&mut self, id: NodeId, value: String) -> Result<(), DomError> {
        if self.html_local_is(id, "input") {
            return self.set_attribute(id, "value", value);
        }
        if !self.html_local_is(id, "textarea") {
            return Err(DomError::WrongNodeType);
        }
        let replacement = self.create_fragment();
        if !value.is_empty() {
            let text = self.create_text(value);
            self.append(replacement, text)?;
        }
        self.replace_all(id, replacement)
    }

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
            let in_range = |candidate: f64| {
                candidate >= min - 1e-9 && (max < min || candidate <= max + 1e-9)
            };
            let mut best: Option<f64> = None;
            for candidate in [base + quotient.floor() * step, base + quotient.ceil() * step] {
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

