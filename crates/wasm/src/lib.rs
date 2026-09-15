//! Browser-loadable tinybrowser page engine component.

use std::cell::RefCell;
use std::sync::Arc;

use renderer::{DialCompletion, DialFailure, DialRequest, EmbeddedRenderer, EngineHost, Mount};
use url::Url;

#[expect(
    unsafe_code,
    clippy::same_length_and_capacity,
    clippy::mem_forget,
    reason = "wit-bindgen maintains the generated component ABI implementation"
)]
mod bindings {
    use super::Component;

    wit_bindgen::generate!({ world: "demo" });
    export!(Component with_types_in self);
}

use bindings::exports::tinybrowser::demo::engine::{Guest, GuestTab};

struct Component;

impl Guest for Component {
    type Tab = Tab;
}

struct Tab {
    renderer: RefCell<Option<EmbeddedRenderer>>,
    mount_error: Option<String>,
}

impl GuestTab for Tab {
    fn new(url: String, html: String) -> Self {
        let mut renderer = EmbeddedRenderer::new(Arc::new(DemoHost));
        let mount = Mount {
            url,
            content_type: Some("text/html; charset=utf-8".into()),
            content_language: None,
            body: html.into_bytes(),
        };
        match renderer.mount(&mount) {
            Ok(()) => Self {
                renderer: RefCell::new(Some(renderer)),
                mount_error: None,
            },
            Err(error) => Self {
                renderer: RefCell::new(None),
                mount_error: Some(error.to_string()),
            },
        }
    }

    fn eval(&self, source: String) -> Result<String, String> {
        self.with_renderer(|renderer| renderer.eval(&source))
    }

    fn pump(&self) {
        if let Some(renderer) = self.renderer.borrow_mut().as_mut() {
            renderer.pump_ready();
        }
    }

    fn title(&self) -> Result<String, String> {
        self.with_renderer(|renderer| renderer.eval("document.title"))
    }

    fn text(&self) -> Result<String, String> {
        self.with_renderer(|renderer| {
            renderer.eval("document.body === null ? '' : document.body.textContent")
        })
    }
}

impl Tab {
    fn with_renderer(
        &self,
        operation: impl FnOnce(&mut EmbeddedRenderer) -> Result<String, renderer::TabError>,
    ) -> Result<String, String> {
        let mut renderer = self.renderer.borrow_mut();
        match renderer.as_mut() {
            Some(renderer) => operation(renderer).map_err(|error| error.to_string()),
            None => Err(self
                .mount_error
                .clone()
                .unwrap_or_else(|| "renderer is unavailable".into())),
        }
    }
}

struct DemoHost;

impl EngineHost for DemoHost {
    fn start_dial(&self, _request: DialRequest, completion: DialCompletion) {
        completion(Err(DialFailure::Connect));
    }

    fn cookies_for(&self, _url: &Url) -> String {
        String::new()
    }

    fn set_cookie(&self, _value: &str, _url: &Url) {}
}
