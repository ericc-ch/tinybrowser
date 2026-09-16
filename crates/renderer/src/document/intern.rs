use std::time::{Duration, Instant as WallClock};

use super::{Document, ScriptValue};
use crate::protocol::TabError;

impl Document {
    pub(crate) fn execute_remote(
        &mut self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<crate::RemoteValue, TabError> {
        let deadline = timeout.map(|duration| WallClock::now() + duration);
        let value = self.execute_script_deadline(source, deadline)?;
        Ok(self.intern_script(value))
    }

    fn intern_script(&mut self, value: ScriptValue) -> crate::RemoteValue {
        match value {
            ScriptValue::Undefined => crate::RemoteValue::Undefined,
            ScriptValue::Null => crate::RemoteValue::Null,
            ScriptValue::Bool(flag) => crate::RemoteValue::Bool(flag),
            ScriptValue::Number(number) => crate::RemoteValue::Number(number),
            ScriptValue::String(text) => crate::RemoteValue::String(text),
            ScriptValue::List(items) => crate::RemoteValue::List(
                items
                    .into_iter()
                    .map(|item| self.intern_script(item))
                    .collect(),
            ),
            ScriptValue::Map(entries) => crate::RemoteValue::Map(
                entries
                    .into_iter()
                    .map(|(key, item)| (key, self.intern_script(item)))
                    .collect(),
            ),
            ScriptValue::Node(id) => {
                crate::RemoteValue::Node(self.world.borrow_mut().remote_id(id))
            }
        }
    }
}
