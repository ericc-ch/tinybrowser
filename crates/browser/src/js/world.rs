//! Shared page world for the JS host.
//!
//! `JsLifetime` is an unsafe rquickjs trait. Persistents are lifetime-erased
//! and dropped in `clear_listeners` before the `QuickJS` runtime.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dom::NodeId;
use net::Agent;
use rquickjs::{Persistent, Value, function::Function};
use url::Url;

use crate::Parsed;

pub(crate) struct Listener {
    pub typ: String,
    pub callback: Persistent<Function<'static>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EventTargetKey {
    Window,
    Node(NodeId),
}

pub(crate) struct World {
    pub parsed: Option<Parsed>,
    pub document_url: Url,
    pub agent: Agent,
    pub pending_cancels: Vec<i32>,
    pub document_ready: bool,
    listeners: HashMap<EventTargetKey, Vec<Listener>>,
    wrappers: HashMap<NodeId, Persistent<Value<'static>>>,
}

impl World {
    pub(crate) fn new(agent: Agent, document_url: Url) -> Self {
        Self {
            parsed: None,
            document_url,
            agent,
            pending_cancels: Vec::new(),
            document_ready: false,
            listeners: HashMap::new(),
            wrappers: HashMap::new(),
        }
    }

    pub(crate) fn replace_document(&mut self, parsed: Parsed) {
        self.parsed = Some(parsed);
        self.document_ready = false;
        self.listeners.clear();
        self.wrappers.clear();
    }

    pub(crate) fn add_listener(&mut self, target: EventTargetKey, listener: Listener) {
        self.listeners.entry(target).or_default().push(listener);
    }

    pub(crate) fn clear_listeners(&mut self) {
        self.listeners.clear();
        self.wrappers.clear();
    }

    pub(crate) fn wrapper(&self, id: NodeId) -> Option<Persistent<Value<'static>>> {
        self.wrappers.get(&id).cloned()
    }

    pub(crate) fn intern_wrapper(&mut self, id: NodeId, value: Persistent<Value<'static>>) {
        self.wrappers.insert(id, value);
    }

    pub(crate) fn listeners(
        &self,
        target: EventTargetKey,
        typ: &str,
    ) -> Vec<Persistent<Function<'static>>> {
        self.listeners
            .get(&target)
            .into_iter()
            .flatten()
            .filter(|listener| listener.typ == typ)
            .map(|listener| listener.callback.clone())
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct SharedWorld(pub Rc<RefCell<World>>);

// SAFETY: `SharedWorld` is an `Rc` to page state. Listener and wrapper
// persistents are dropped in `clear_listeners` / `replace_document` before
// `JsHost` drops the runtime they were saved from.
unsafe impl rquickjs::JsLifetime<'_> for SharedWorld {
    type Changed<'to> = SharedWorld;
}
