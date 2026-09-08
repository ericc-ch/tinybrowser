//! Shared page world for the JS host.
//!
//! `JsLifetime` is an unsafe rquickjs trait. `SharedWorld` holds no JS
//! pointers, so `Changed<'to> = Self` is sound.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use dom::NodeId;
use net::Agent;
use rquickjs::{Persistent, function::Function};
use url::Url;

use crate::Parsed;

pub(crate) struct Listener {
    pub typ: String,
    pub callback: Persistent<Function<'static>>,
}

#[derive(Clone, Copy)]
pub(crate) enum EventTargetKey {
    Window,
    Node(NodeId),
}

pub(crate) struct World {
    pub parsed: Option<Parsed>,
    pub document_url: Url,
    pub agent: Agent,
    pub pending_cancels: Vec<i32>,
    /// `document.readyState` is `complete` after `load`.
    pub document_ready: bool,
    listeners: HashMap<ListenerKey, Vec<Listener>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ListenerKey {
    Window,
    Node(NodeId),
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
        }
    }

    pub(crate) fn replace_document(&mut self, parsed: Parsed) {
        self.parsed = Some(parsed);
        self.document_ready = false;
        self.listeners.clear();
    }

    pub(crate) fn add_listener(&mut self, target: EventTargetKey, listener: Listener) {
        self.listeners
            .entry(listener_key(target))
            .or_default()
            .push(listener);
    }

    pub(crate) fn clear_listeners(&mut self) {
        self.listeners.clear();
    }

    pub(crate) fn listeners(
        &self,
        target: EventTargetKey,
        typ: &str,
    ) -> Vec<Persistent<Function<'static>>> {
        self.listeners
            .get(&listener_key(target))
            .into_iter()
            .flatten()
            .filter(|listener| listener.typ == typ)
            .map(|listener| listener.callback.clone())
            .collect()
    }
}

fn listener_key(target: EventTargetKey) -> ListenerKey {
    match target {
        EventTargetKey::Window => ListenerKey::Window,
        EventTargetKey::Node(id) => ListenerKey::Node(id),
    }
}

#[derive(Clone)]
pub(crate) struct SharedWorld(pub Rc<RefCell<World>>);

// SAFETY: `SharedWorld` is an `Rc` to Rust page state; it holds no JS values.
unsafe impl rquickjs::JsLifetime<'_> for SharedWorld {
    type Changed<'to> = SharedWorld;
}
