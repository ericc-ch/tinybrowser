//! Domain reactions at the tree and attribute mutation boundaries.

use crate::{Dom, DomError, NodeId};

impl Dom {
    pub(crate) fn node_inserted(&mut self, node: NodeId) {
        self.index_inserted_names(node);
    }

    pub(crate) fn attribute_changed(&mut self, node: NodeId, name: &str) {
        self.index_changed_name(node, name);
    }

    pub(crate) fn attribute_set(&mut self, node: NodeId, name: &str) -> Result<(), DomError> {
        self.attribute_changed(node, name);
        if name.eq_ignore_ascii_case("selected") {
            self.refresh_option_selectedness(node);
            if let Some(select) = self.option_select_owner(node) {
                self.apply_default_selectedness(select);
            }
        }
        if name.eq_ignore_ascii_case("type") {
            self.refresh_input_type(node)?;
        }
        if name.eq_ignore_ascii_case("name") || name.eq_ignore_ascii_case("checked") {
            self.refresh_radio_group(node);
        }
        if self.html_local_is(node, "select")
            && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
        {
            self.apply_default_selectedness(node);
        }
        Ok(())
    }

    pub(crate) fn attribute_removed(&mut self, node: NodeId, name: &str) -> Result<(), DomError> {
        self.attribute_changed(node, name);
        if name.eq_ignore_ascii_case("selected") {
            self.refresh_option_selectedness(node);
            if let Some(select) = self.option_select_owner(node) {
                self.apply_default_selectedness(select);
            }
        }
        if name.eq_ignore_ascii_case("type") {
            self.refresh_input_type(node)?;
        }
        if self.html_local_is(node, "select")
            && (name.eq_ignore_ascii_case("multiple") || name.eq_ignore_ascii_case("size"))
        {
            self.apply_default_selectedness(node);
        }
        Ok(())
    }
}
