//! Form controls: forms, inputs, text areas, selects, options, buttons,
//! and their value/selection state.

use super::super::{
    CollectionKind, LegacyNullString, WebIdlCodeUnits, WebIdlString, WebIdlUnsignedLong,
    document_base_url_string, host, live_collection, remove_attribute_sync,
    schedule_mutation_delivery, set_attribute_sync, throw_dom, with_node_data, world,
    world_for_node, wrap_node,
};

use rquickjs::{Ctx, Exception, Result, Value};

use crate::dom::insertion::replace_all_journaled;
use crate::dom::tree::{NodeId, attr, form_owner_of, html_namespace};

use super::JsNode;
use super::element::{content_attr, reflect_enum, reflect_ulong};
use super::media::is_image_input;

include!(concat!(env!("OUT_DIR"), "/HTMLFormElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLInputElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLTextAreaElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLSelectElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOptionElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLButtonElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLFieldSetElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOptGroupElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLOutputElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLDataListElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLLabelElement.rs"));
include!(concat!(env!("OUT_DIR"), "/HTMLLegendElement.rs"));

impl<'js> html_form_element_generated::HTMLFormElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    fn get_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    fn get_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let method = reflect_enum(
            content_attr(ctx, self.handle.0, "method").as_deref(),
            &["post", "dialog"],
            "get",
            "get",
        );
        rquickjs::String::from_str(ctx.clone(), &method)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-method
    fn set_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_attribute(
            ctx.clone(),
            WebIdlString("method".into()),
            WebIdlString(value.to_string()?),
        )
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    fn get_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let enctype = reflect_enum(
            content_attr(ctx, self.handle.0, "enctype").as_deref(),
            &["multipart/form-data", "text/plain"],
            "application/x-www-form-urlencoded",
            "application/x-www-form-urlencoded",
        );
        rquickjs::String::from_str(ctx.clone(), &enctype)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-enctype
    fn set_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_attribute(
            ctx.clone(),
            WebIdlString("enctype".into()),
            WebIdlString(value.to_string()?),
        )
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-encoding
    fn get_encoding(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        self.get_enctype(ctx)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-encoding
    fn set_encoding(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_enctype(ctx, value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    fn reset(&self, ctx: Ctx<'js>) -> Result<()> {
        self.reset_form(ctx)
    }

    // `autocomplete` is an on/off enumeration defaulting to on
    // (<https://html.spec.whatwg.org/multipage/forms.html#dom-form-autocomplete>).
    fn get_autocomplete(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        const KEYWORDS: &[&str] = &["on", "off"];
        let value = reflect_enum(
            content_attr(ctx, self.handle.0, "autocomplete").as_deref(),
            KEYWORDS,
            "on",
            "on",
        );
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-autocomplete
    fn set_autocomplete(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        set_attribute_sync(ctx, self.handle.0, "autocomplete", &value.to_string()?)
    }
}

impl<'js> html_input_element_generated::HTMLInputElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    fn get_checked(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.checked(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    fn set_checked(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        self.set_checked(ctx.clone(), value)
    }

    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    fn get_form_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.form_action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn get_form_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let method = self.form_method(ctx.clone());
        rquickjs::String::from_str(ctx.clone(), &method)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn set_form_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_method(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn get_form_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let enctype = self.form_enctype(ctx.clone());
        rquickjs::String::from_str(ctx.clone(), &enctype)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn set_form_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_enctype(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    fn get_indeterminate(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.indeterminate_flag(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    fn set_indeterminate(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        self.set_indeterminate_flag(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // `size` defaults to 20; zero or unparseable reads 20, setting zero
    // throws
    // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-size>).
    fn get_size(&self, ctx: &Ctx<'js>) -> Result<usize> {
        let value = reflect_ulong(content_attr(ctx, self.handle.0, "size").as_deref(), 20);
        Ok(usize::try_from(if value == 0 { 20 } else { value }).unwrap_or(usize::MAX))
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-size
    fn set_size(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        if value == 0 {
            return Err(throw_dom(ctx, "IndexSizeError", "input size is zero"));
        }
        // Out-of-range values write the default rather than the raw value.
        host::reflect_set_ulong_defaulting(ctx, self.handle.0, "size", value, 20)
    }

    // Image-button dimensions default to zero, and read zero outside the
    // image state; the setters are generated from `[ReflectSetter]`
    // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-width>).
    fn get_width(&self, ctx: &Ctx<'js>) -> Result<u32> {
        if !is_image_input(ctx, self.handle.0) {
            return Ok(0);
        }
        Ok(reflect_ulong(
            content_attr(ctx, self.handle.0, "width").as_deref(),
            0,
        ))
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-height
    fn get_height(&self, ctx: &Ctx<'js>) -> Result<u32> {
        if !is_image_input(ctx, self.handle.0) {
            return Ok(0);
        }
        Ok(reflect_ulong(
            content_attr(ctx, self.handle.0, "height").as_deref(),
            0,
        ))
    }

    // `autocomplete` reflects with a generated setter; the getter answers
    // the content attribute
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
    fn get_autocomplete(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        host::reflect_string(ctx, self.handle.0, "autocomplete")
    }

    // Upstream selection accessors are nullable, keeping the spec-prose
    // null for input types where selection does not apply. No selection state
    // is stored (a known cutover gap), so applicable inputs answer zero.
    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#textFieldSelection

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn get_selection_start(&self, ctx: &Ctx<'js>) -> Result<Option<u32>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        Ok(Some(selection_start_of(ctx, self.handle.0)?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn set_selection_start(&self, ctx: &Ctx<'js>, value: Option<u32>) -> Result<()> {
        // Null converts as zero, matching the previous numeric conversion.
        self.set_selection_start(ctx.clone(), WebIdlUnsignedLong(value.unwrap_or(0)))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn get_selection_end(&self, ctx: &Ctx<'js>) -> Result<Option<u32>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        Ok(Some(selection_end_of(ctx, self.handle.0)?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn set_selection_end(&self, ctx: &Ctx<'js>, value: Option<u32>) -> Result<()> {
        // Null converts as zero, matching the previous numeric conversion.
        self.set_selection_end(ctx.clone(), WebIdlUnsignedLong(value.unwrap_or(0)))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn get_selection_direction(&self, ctx: &Ctx<'js>) -> Result<Option<rquickjs::String<'js>>> {
        let world = world(ctx)?;
        let parsed = world.borrow();
        let Some(parsed) = parsed.document(self.handle.0) else {
            return Ok(None);
        };
        if !crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node) {
            return Ok(None);
        }
        drop(parsed);
        let direction = selection_direction_of(ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx.clone(), direction_name(direction)).map(Some)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn set_selection_direction(
        &self,
        ctx: &Ctx<'js>,
        value: Option<rquickjs::String<'js>>,
    ) -> Result<()> {
        // Null stringified to "null" before, which maps to no direction;
        // answer "none" directly.
        let value = match value {
            Some(value) => value.to_string()?,
            None => "none".into(),
        };
        self.set_selection_direction(ctx.clone(), WebIdlString(value))
    }
}

impl<'js> html_text_area_element_generated::HTMLTextAreaElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    fn get_default_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.default_value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    fn set_default_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_default_value(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-textlength
    fn get_text_length(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(self.text_length(ctx.clone())? as usize)
    }

    // Selection getters are non-nullable upstream, answering the stored
    // range clamped to the control value.
    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#textFieldSelection

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn get_selection_start(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(selection_start_of(ctx, self.handle.0)? as usize)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    fn set_selection_start(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        self.set_selection_start(ctx.clone(), WebIdlUnsignedLong(value))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn get_selection_end(&self, ctx: &Ctx<'js>) -> Result<usize> {
        Ok(selection_end_of(ctx, self.handle.0)? as usize)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    fn set_selection_end(&self, ctx: &Ctx<'js>, value: u32) -> Result<()> {
        self.set_selection_end(ctx.clone(), WebIdlUnsignedLong(value))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn get_selection_direction(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let direction = selection_direction_of(ctx, self.handle.0)?;
        rquickjs::String::from_str(ctx.clone(), direction_name(direction))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectiondirection
    fn set_selection_direction(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_selection_direction(ctx.clone(), WebIdlString(value.to_string()?))
    }

    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // `autocomplete` reflects with a generated setter; the getter answers
    // the content attribute
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
    fn get_autocomplete(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        host::reflect_string(ctx, self.handle.0, "autocomplete")
    }
}

impl<'js> html_select_element_generated::HTMLSelectElement<'js> for JsNode {
    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-value
    fn set_value(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_value(ctx.clone(), LegacyNullString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn get_selected_index(&self, ctx: &Ctx<'js>) -> Result<i32> {
        self.selected_index(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    fn set_selected_index(&self, ctx: &Ctx<'js>, value: i32) -> Result<()> {
        self.set_selected_index(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-options
    fn get_options(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.options(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedoptions
    fn get_selected_options(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.selected_options(ctx.clone())
    }

    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // `autocomplete` reflects with a generated setter; the getter answers
    // the content attribute
    // (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#attr-fe-autocomplete>).
    fn get_autocomplete(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        host::reflect_string(ctx, self.handle.0, "autocomplete")
    }
}

impl<'js> html_option_element_generated::HTMLOptionElement<'js> for JsNode {
    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-label
    fn get_label(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let label = self.label(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &label)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    fn get_selected(&self, ctx: &Ctx<'js>) -> Result<bool> {
        self.selected(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    fn set_selected(&self, ctx: &Ctx<'js>, value: bool) -> Result<()> {
        self.set_selected(ctx.clone(), value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-value
    fn get_value(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let value = self.value(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &value)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    fn get_text(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let text = self.text(ctx.clone())?;
        rquickjs::String::from_str(ctx.clone(), &text)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    fn set_text(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        // DOMString keeps every code unit, including lone surrogates.
        self.set_text(
            ctx.clone(),
            WebIdlCodeUnits(crate::dom_string::DomString::from_utf16(value.to_utf16()?)),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    fn get_index(&self, ctx: &Ctx<'js>) -> Result<i32> {
        self.index(ctx.clone())
    }
}

impl<'js> html_button_element_generated::HTMLButtonElement<'js> for JsNode {
    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    fn get_form_action(&self, ctx: &Ctx<'js>) -> Result<crate::dom_string::DomString> {
        Ok(self.form_action(ctx.clone())?.into())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn get_form_method(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let method = self.form_method(ctx.clone());
        rquickjs::String::from_str(ctx.clone(), &method)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    fn set_form_method(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_method(ctx.clone(), WebIdlString(value.to_string()?))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn get_form_enctype(&self, ctx: &Ctx<'js>) -> Result<rquickjs::String<'js>> {
        let enctype = self.form_enctype(ctx.clone());
        rquickjs::String::from_str(ctx.clone(), &enctype)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    fn set_form_enctype(&self, ctx: &Ctx<'js>, value: rquickjs::String<'js>) -> Result<()> {
        self.set_form_enctype(ctx.clone(), WebIdlString(value.to_string()?))
    }
}

impl<'js> html_field_set_element_generated::HTMLFieldSetElement<'js> for JsNode {
    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }
}

impl html_opt_group_element_generated::HTMLOptGroupElement<'_> for JsNode {}

impl<'js> html_output_element_generated::HTMLOutputElement<'js> for JsNode {
    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    fn get_form(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        self.form(ctx.clone())
    }
}

impl html_data_list_element_generated::HTMLDataListElement<'_> for JsNode {}

impl html_label_element_generated::HTMLLabelElement<'_> for JsNode {}

impl html_legend_element_generated::HTMLLegendElement<'_> for JsNode {}

#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value; create* is a document method"
)]
impl JsNode {
    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    //
    // Known gap: Blitz keeps no control value store (no dirty value flag, no
    // checkedness or selectedness slots), so every value here is read from
    // content attributes and descendant text: an input's value is its `value`
    // attribute, a textarea's is its descendant text, and a select's comes
    // from the selected option.
    pub(super) fn value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        let base = &parsed.document.base;
        let Some(element) = base
            .get_node(self.handle.0.node)
            .and_then(|node| node.data.downcast_element())
        else {
            return Ok(String::new());
        };
        if element.name.ns != html_namespace() {
            return Ok(String::new());
        }
        Ok(match element.name.local.as_str() {
            "textarea" => {
                if element.form_state.dirty_value {
                    element.form_state.value.clone()
                } else {
                    crate::dom::text::descendant_text(&parsed.document.base, self.handle.0.node)
                        .to_string_lossy()
                        .into_owned()
                }
            }
            "select" => {
                crate::dom::forms::select_value(base, self.handle.0.document, self.handle.0.node)
            }
            "option" => crate::dom::forms::option_value(base, self.handle.0.node),
            "input" if value_is_content_attribute(base, self.handle.0.node) => {
                attr(base, self.handle.0.node, "value")
                    .unwrap_or_default()
                    .to_owned()
            }
            _ => element.control_value().to_owned(),
        })
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-value
    pub(super) fn set_value(&self, ctx: Ctx<'_>, value: LegacyNullString) -> Result<()> {
        let local = with_node_data(&ctx, self.handle.0, |data| {
            data.and_then(|data| data.downcast_element())
                .filter(|element| element.name.ns == html_namespace())
                .map(|element| element.name.local.to_string())
        })?;
        let Some(local) = local else {
            return Ok(());
        };
        match local.as_str() {
            "textarea" => self.set_textarea_value(&ctx, value.0),
            "select" => self.set_select_value(&ctx, value.0),
            "input" => {
                let reflected = {
                    let world = world(&ctx)?;
                    let world = world.borrow();
                    world.document(self.handle.0).is_some_and(|parsed| {
                        value_is_content_attribute(&parsed.document.base, self.handle.0.node)
                    })
                };
                if reflected {
                    set_attribute_sync(&ctx, self.handle.0, "value", &value.0)
                } else {
                    let world = world(&ctx)?;
                    let world = world.borrow();
                    let Some(mut parsed) = world.document_mut(self.handle.0) else {
                        return Ok(());
                    };
                    parsed
                        .document
                        .base
                        .mutate()
                        .set_dirty_value(self.handle.0.node, value.0);
                    Ok(())
                }
            }
            _ => set_attribute_sync(&ctx, self.handle.0, "value", &value.0),
        }
    }

    /// Sets a textarea's dirty value. The children stay the default value
    /// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-value>).
    pub(super) fn set_textarea_value(&self, ctx: &Ctx<'_>, value: String) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        parsed
            .document
            .base
            .mutate()
            .set_dirty_value(self.handle.0.node, value);
        Ok(())
    }

    /// Replaces a textarea's children. `defaultValue` uses this.
    pub(super) fn set_textarea_default_value(&self, ctx: &Ctx<'_>, value: String) -> Result<()> {
        let world = world(ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Err(Exception::throw_type(ctx, "no document"));
        };
        let added = if value.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&value);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        let iframes = replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        world.destroy_child_navigables(&iframes);
        drop(world);
        schedule_mutation_delivery(ctx)
    }

    /// Selects the first option whose value is `value`, deselecting the rest;
    /// with no match every option is deselected.
    pub(super) fn set_select_value(&self, ctx: &Ctx<'_>, value: String) -> Result<()> {
        let world_rc = world(ctx)?;
        let options = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Err(Exception::throw_type(ctx, "no document"));
            };
            let base = &parsed.document.base;
            crate::dom::forms::select_options(base, self.handle.0.document, self.handle.0.node)
        };
        let mut matched = false;
        for option in options {
            let selected = !matched && {
                let world = world_rc.borrow();
                let Some(parsed) = world.document(option) else {
                    continue;
                };
                crate::dom::forms::option_value(&parsed.document.base, option.node) == value
            };
            if selected {
                matched = true;
            }
            let world = world_rc.borrow();
            let Some(mut parsed) = world.document_mut(option) else {
                continue;
            };
            parsed
                .document
                .base
                .mutate()
                .set_selectedness(option.node, selected);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    pub(super) fn default_value(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Err(Exception::throw_type(&ctx, "no document"));
        };
        Ok(
            crate::dom::text::descendant_text(&parsed.document.base, self.handle.0.node)
                .to_string_lossy()
                .into_owned(),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-defaultvalue
    pub(super) fn set_default_value(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_textarea_default_value(&ctx, value.0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-textarea-textlength
    pub(super) fn text_length(&self, ctx: Ctx<'_>) -> Result<u32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a 64-bit string length beyond u32 cannot be produced by this engine"
        )]
        Ok(
            crate::dom::text::descendant_text(&parsed.document.base, self.handle.0.node)
                .to_string_lossy()
                .encode_utf16()
                .count() as u32,
        )
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionstart
    pub(super) fn set_selection_start(
        &self,
        ctx: Ctx<'_>,
        value: WebIdlUnsignedLong,
    ) -> Result<()> {
        let world_rc = world(&ctx)?;
        let (applies, len, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node),
                crate::dom::forms::control_value_len(&parsed.document.base, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionStart does not apply to this control",
            ));
        }
        let start = value.0.min(len);
        let (_, end, direction) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start, end.max(start), direction);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-textarea/input-selectionend
    pub(super) fn set_selection_end(&self, ctx: Ctx<'_>, value: WebIdlUnsignedLong) -> Result<()> {
        let world_rc = world(&ctx)?;
        let (applies, len, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node),
                crate::dom::forms::control_value_len(&parsed.document.base, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionEnd does not apply to this control",
            ));
        }
        let end = value.0.min(len);
        let (start, _, direction) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start.min(end), end, direction);
        Ok(())
    }

    pub(super) fn set_selection_direction(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        let direction = direction_code(&value.0);
        let world_rc = world(&ctx)?;
        let (applies, current) = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            (
                crate::dom::forms::selection_applies(&parsed.document.base, self.handle.0.node),
                world.selection_of(self.handle.0),
            )
        };
        if !applies {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "selectionDirection does not apply to this control",
            ));
        }
        let (start, end, _) = current;
        world_rc
            .borrow_mut()
            .set_selection(self.handle.0, start, end, direction);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-reset
    pub(super) fn reset_form(&self, ctx: Ctx<'_>) -> Result<()> {
        let owner = world_for_node(&ctx, self.handle.0)?;
        let is_form = owner
            .borrow()
            .document(self.handle.0)
            .is_some_and(|parsed| {
                crate::dom::tree::is_html_element(&parsed.document.base, self.handle.0.node, "form")
            });
        if !is_form {
            return Err(throw_dom(
                &ctx,
                "InvalidStateError",
                "reset is only available on a form",
            ));
        }
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let controls = form_controls(&parsed.document.base, parsed.id, self.handle.0.node);
        let mut mutator = parsed.document.base.mutate();
        for control in controls {
            mutator.reset_control(control);
        }
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/forms.html#dom-form-action
    pub(super) fn action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world_rc = world(&ctx)?;
        let raw = world_rc
            .borrow()
            .document(self.handle.0)
            .and_then(|parsed| {
                attr(&parsed.document.base, self.handle.0.node, "action").map(ToOwned::to_owned)
            });
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formaction
    pub(super) fn form_action(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let raw = world.borrow().document(self.handle.0).and_then(|parsed| {
            attr(&parsed.document.base, self.handle.0.node, "formaction").map(ToOwned::to_owned)
        });
        let base = document_base_url_string(&ctx, self.handle.0);
        let Some(raw) = raw else {
            return Ok(base);
        };
        Ok(url::Url::parse(&base)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map_or(raw, |url| url.to_string()))
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formmethod
    pub(super) fn form_method(&self, ctx: Ctx<'_>) -> String {
        // Enumerated without whitespace stripping: only an exact ASCII
        // case-insensitive keyword matches
        // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).
        reflect_enum(
            content_attr(&ctx, self.handle.0, "formmethod").as_deref(),
            &["post", "dialog"],
            "",
            "get",
        )
    }

    pub(super) fn set_form_method(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formmethod".into()), value)
    }

    // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#dom-fs-formenctype
    pub(super) fn form_enctype(&self, ctx: Ctx<'_>) -> String {
        // Absent reads empty, like `formMethod`; no whitespace stripping
        // (<https://html.spec.whatwg.org/multipage/common-dom-interfaces.html#limited-to-only-known-values>).
        reflect_enum(
            content_attr(&ctx, self.handle.0, "formenctype").as_deref(),
            &["multipart/form-data", "text/plain"],
            "",
            "application/x-www-form-urlencoded",
        )
    }

    pub(super) fn set_form_enctype(&self, ctx: Ctx<'_>, value: WebIdlString) -> Result<()> {
        self.set_attribute(ctx, WebIdlString("formenctype".into()), value)
    }

    /// <https://html.spec.whatwg.org/multipage/forms.html#dom-fae-form>
    pub(super) fn form<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let form = {
            let world = world(&ctx)?;
            let world = world.borrow();
            world.document(self.handle.0).and_then(|parsed| {
                form_owner_of(
                    &parsed.document.base,
                    self.handle.0.document,
                    self.handle.0.node,
                )
            })
        };
        match form {
            Some(form) => wrap_node(&ctx, form),
            None => Ok(Value::new_null(ctx)),
        }
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-checked
    pub(super) fn checked(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).is_some_and(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.data.downcast_element())
                .is_some_and(blitz_dom::ElementData::checkedness)
        }))
    }

    pub(super) fn set_checked(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        // Setting a radio's checkedness unchecks its group. The content
        // attribute is the default and is left alone
        // (<https://html.spec.whatwg.org/multipage/input.html#dom-input-checked>).
        let group = if value {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            let base = &parsed.document.base;
            let is_radio = base
                .get_node(self.handle.0.node)
                .and_then(|node| node.data.downcast_element())
                .is_some_and(|element| {
                    element.name.ns == html_namespace()
                        && element.name.local.as_str() == "input"
                        && attr(base, self.handle.0.node, "type")
                            .unwrap_or("text")
                            .trim()
                            .eq_ignore_ascii_case("radio")
                });
            if is_radio {
                let name = attr(base, self.handle.0.node, "name")
                    .unwrap_or_default()
                    .to_owned();
                crate::dom::forms::radio_group_members(
                    base,
                    self.handle.0.document,
                    &name,
                    self.handle.0.node,
                )
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let mut mutator = parsed.document.base.mutate();
        for member in group {
            mutator.set_checkedness(member.node, false);
        }
        mutator.set_checkedness(self.handle.0.node, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/input.html#dom-input-indeterminate
    pub(super) fn indeterminate_flag(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).is_some_and(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.data.downcast_element())
                .is_some_and(|element| element.form_state.indeterminate)
        }))
    }

    pub(super) fn set_indeterminate_flag(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        parsed
            .document
            .base
            .mutate()
            .set_indeterminate(self.handle.0.node, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected
    pub(super) fn selected(&self, ctx: Ctx<'_>) -> Result<bool> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).is_some_and(|parsed| {
            parsed
                .document
                .base
                .get_node(self.handle.0.node)
                .and_then(|node| node.data.downcast_element())
                .is_some_and(blitz_dom::ElementData::selectedness)
        }))
    }

    pub(super) fn set_selected(&self, ctx: Ctx<'_>, value: bool) -> Result<()> {
        // Selecting an option in a single-select deselects the others. The
        // content attribute stays the default selectedness
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
        let siblings = if value {
            let world = world(&ctx)?;
            let world = world.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            let base = &parsed.document.base;
            match crate::dom::forms::option_select_owner(base, self.handle.0.node).map(|node| {
                NodeId {
                    document: self.handle.0.document,
                    node,
                }
            }) {
                Some(owner) if attr(base, owner.node, "multiple").is_none() => {
                    crate::dom::forms::select_options(base, self.handle.0.document, owner.node)
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let mut mutator = parsed.document.base.mutate();
        for option in siblings {
            if option != self.handle.0 {
                mutator.set_selectedness(option.node, false);
            }
        }
        mutator.set_selectedness(self.handle.0.node, value);
        Ok(())
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-label
    pub(super) fn label(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                let base = &parsed.document.base;
                attr(base, self.handle.0.node, "label").map_or_else(
                    || crate::dom::forms::option_text(base, self.handle.0.node),
                    str::to_owned,
                )
            }))
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text
    pub(super) fn text(&self, ctx: Ctx<'_>) -> Result<String> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world
            .document(self.handle.0)
            .map_or_else(String::new, |parsed| {
                crate::dom::forms::option_text(&parsed.document.base, self.handle.0.node)
            }))
    }

    pub(super) fn set_text(&self, ctx: Ctx<'_>, value: WebIdlCodeUnits) -> Result<()> {
        // An option's text setter replaces its children with one text node
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-text>).
        let text = value.0.to_string_lossy().into_owned();
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let added = if text.is_empty() {
            Vec::new()
        } else {
            let text = parsed.document.base.mutate().create_text_node(&text);
            vec![NodeId {
                document: parsed.id,
                node: text,
            }]
        };
        let iframes = replace_all_journaled(&mut parsed, self.handle.0, added);
        drop(parsed);
        world.destroy_child_navigables(&iframes);
        drop(world);
        schedule_mutation_delivery(&ctx)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-index
    pub(super) fn index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        let Some(parsed) = world.document(self.handle.0) else {
            return Ok(0);
        };
        let base = &parsed.document.base;
        let Some(select) =
            crate::dom::forms::option_select_owner(base, self.handle.0.node).map(|node| NodeId {
                document: self.handle.0.document,
                node,
            })
        else {
            return Ok(0);
        };
        for (index, option) in
            crate::dom::forms::select_options(base, self.handle.0.document, select.node)
                .into_iter()
                .enumerate()
        {
            if option == self.handle.0 {
                return Ok(i32::try_from(index).unwrap_or(i32::MAX));
            }
        }
        Ok(0)
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex
    pub(super) fn selected_index(&self, ctx: Ctx<'_>) -> Result<i32> {
        let world = world(&ctx)?;
        let world = world.borrow();
        Ok(world.document(self.handle.0).map_or(-1, |parsed| {
            let base = &parsed.document.base;
            crate::dom::forms::select_options(base, self.handle.0.document, self.handle.0.node)
                .iter()
                .position(|option| {
                    base.get_node(option.node)
                        .and_then(|node| node.data.downcast_element())
                        .is_some_and(blitz_dom::ElementData::selectedness)
                })
                .map_or(-1, |index| i32::try_from(index).unwrap_or(-1))
        }))
    }

    pub(super) fn set_selected_index(&self, ctx: Ctx<'_>, value: i32) -> Result<()> {
        let world_rc = world(&ctx)?;
        let options = {
            let world = world_rc.borrow();
            let Some(parsed) = world.document(self.handle.0) else {
                return Ok(());
            };
            crate::dom::forms::select_options(
                &parsed.document.base,
                self.handle.0.document,
                self.handle.0.node,
            )
        };
        // A negative index clears every option; otherwise the indexed option
        // selects and the rest deselect
        // (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedindex>).
        let wanted = if value < 0 {
            None
        } else {
            usize::try_from(value).ok()
        };
        let world = world_rc.borrow();
        let Some(mut parsed) = world.document_mut(self.handle.0) else {
            return Ok(());
        };
        let mut mutator = parsed.document.base.mutate();
        for (index, option) in options.into_iter().enumerate() {
            mutator.set_selectedness(option.node, Some(index) == wanted);
        }
        Ok(())
    }

    pub(super) fn options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::SelectOptions,
            Some("HTMLOptionsCollection"),
        )
    }

    // https://html.spec.whatwg.org/multipage/form-elements.html#dom-select-selectedoptions
    pub(super) fn selected_options<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        live_collection(
            &ctx,
            self.handle.0,
            CollectionKind::SelectedOptions,
            Some("HTMLCollection"),
        )
    }
}
/// An inserted selected option clears a single-select owner's other options
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#dom-option-selected>).
/// Blitz keeps selectedness as the `selected` content attribute, so the rule
/// rewrites attributes instead of slots.
pub(super) fn fixup_option_on_insert(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    let siblings = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Ok(());
        };
        crate::dom::forms::options_to_deselect(&parsed.document.base, id.document, id.node)
    };
    for option in siblings {
        remove_attribute_sync(ctx, option, "", "selected", false)?;
    }
    Ok(())
}

/// An inserted checked radio unchecks the other radios it joins.
pub(super) fn fixup_radio_on_insert(ctx: &Ctx<'_>, id: NodeId) -> Result<()> {
    let world_rc = world(ctx)?;
    let group = {
        let world = world_rc.borrow();
        let Some(parsed) = world.document(id) else {
            return Ok(());
        };
        crate::dom::forms::radios_to_uncheck(&parsed.document.base, id.document, id.node)
    };
    for member in group {
        remove_attribute_sync(ctx, member, "", "checked", false)?;
    }
    Ok(())
}

/// A control's stored selection start clamped to its value length.
pub(super) fn selection_start_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u32> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let parsed = world.document(id);
    let Some(parsed) = parsed else {
        return Ok(0);
    };
    let (start, _, _) = world.selection_of(id);
    Ok(start.min(crate::dom::forms::control_value_len(
        &parsed.document.base,
        id.node,
    )))
}

/// A control's stored selection end clamped to its value length.
pub(super) fn selection_end_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u32> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(0);
    };
    let (_, end, _) = world.selection_of(id);
    Ok(end.min(crate::dom::forms::control_value_len(
        &parsed.document.base,
        id.node,
    )))
}

/// A control's stored selection direction code.
pub(super) fn selection_direction_of(ctx: &rquickjs::Ctx<'_>, id: NodeId) -> rquickjs::Result<u8> {
    let world_rc = world(ctx)?;
    let world = world_rc.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(0);
    };
    if !crate::dom::forms::selection_applies(&parsed.document.base, id.node) {
        return Ok(0);
    }
    Ok(world.selection_of(id).2)
}

/// The `SelectionMode` direction name for the stored code
/// (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#selection-direction>).
pub(super) fn direction_name(direction: u8) -> &'static str {
    match direction {
        1 => "forward",
        2 => "backward",
        _ => "none",
    }
}

/// Maps a `selectionDirection` string to its stored code; anything but the two
/// known keywords is "none".
pub(super) fn direction_code(direction: &str) -> u8 {
    match direction {
        "forward" => 1,
        "backward" => 2,
        _ => 0,
    }
}

/// Checkbox, radio, and button-like inputs reflect `value` as a content
/// attribute. Other inputs keep a dirty value apart from that attribute
/// (<https://html.spec.whatwg.org/multipage/input.html#dom-input-value>).
fn value_is_content_attribute(
    base: &blitz_dom::BaseDocument,
    node: blitz_traits::node_id::NodeId,
) -> bool {
    let Some(kind) = attr(base, node, "type") else {
        return false;
    };
    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "checkbox" | "radio" | "file" | "submit" | "image" | "reset" | "button"
    )
}

/// Form-associated elements whose form owner is `form`.
fn form_controls(
    base: &blitz_dom::BaseDocument,
    document: u32,
    form: blitz_traits::node_id::NodeId,
) -> Vec<blitz_traits::node_id::NodeId> {
    let form_id = NodeId {
        document,
        node: form,
    };
    let mut root = form;
    while let Some(parent) = base.get_node(root).and_then(|node| node.parent) {
        root = parent;
    }
    let mut controls = Vec::new();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        stack.extend(node.children.iter().rev().copied());
        let Some(element) = node.data.downcast_element() else {
            continue;
        };
        if element.name.ns != html_namespace() {
            continue;
        }
        if !matches!(
            element.name.local.as_str(),
            "input" | "textarea" | "select" | "button" | "option"
        ) {
            continue;
        }
        let owned = element.name.local.as_str() == "option"
            || form_owner_of(base, document, current) == Some(form_id);
        if owned {
            controls.push(current);
        }
    }
    controls
}

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    html_form_element_generated::install(ctx)?;
    html_input_element_generated::install(ctx)?;
    html_text_area_element_generated::install(ctx)?;
    html_select_element_generated::install(ctx)?;
    html_option_element_generated::install(ctx)?;
    html_button_element_generated::install(ctx)?;
    html_field_set_element_generated::install(ctx)?;
    html_opt_group_element_generated::install(ctx)?;
    html_output_element_generated::install(ctx)?;
    html_data_list_element_generated::install(ctx)?;
    html_label_element_generated::install(ctx)?;
    html_legend_element_generated::install(ctx)?;
    Ok(())
}
