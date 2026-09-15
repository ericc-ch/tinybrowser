//! Browser-loadable tinybrowser page engine component.

use std::cell::RefCell;
use std::sync::Arc;

use renderer::{
    BrowserServices, DialCompletion, DialFailure, DialRequest, EmbeddedRenderer, Mount,
    TabEvent as RendererEvent,
};
use url::Url;

#[expect(
    unsafe_code,
    clippy::same_length_and_capacity,
    clippy::mem_forget,
    reason = "wit-bindgen maintains the generated component ABI implementation"
)]
mod bindings {
    use super::Component;

    wit_bindgen::generate!({ world: "browser" });
    export!(Component with_types_in self);
}

use bindings::exports::tinybrowser::browser::engine::{Event, FrameEvent, Guest, GuestTab};

struct Component;

impl Guest for Component {
    type Tab = Tab;
}

struct Tab {
    renderer: RefCell<EmbeddedRenderer>,
}

impl GuestTab for Tab {
    fn new(url: String, html: String) -> Result<Self, String> {
        let mut renderer = EmbeddedRenderer::new(Arc::new(OfflineHost));
        let mount = Mount {
            url,
            content_type: Some("text/html; charset=utf-8".into()),
            content_language: None,
            body: html.into_bytes(),
        };
        renderer.mount(&mount).map_err(|error| error.to_string())?;
        Ok(Self {
            renderer: RefCell::new(renderer),
        })
    }

    fn eval(&self, source: String) -> Result<String, String> {
        self.with_renderer(|renderer| renderer.eval(&source))
    }

    fn pump(&self) -> Option<u64> {
        let mut renderer = self.renderer.borrow_mut();
        renderer.pump_ready();
        renderer.time_until_deadline().map(|delay| {
            let milliseconds = delay.as_nanos().saturating_add(999_999) / 1_000_000;
            u64::try_from(milliseconds).unwrap_or(u64::MAX)
        })
    }

    fn events(&self) -> Result<Vec<FrameEvent>, String> {
        self.renderer
            .borrow_mut()
            .take_events()
            .map(|events| {
                events
                    .into_iter()
                    .map(|(frame, event)| FrameEvent {
                        frame: frame.get(),
                        event: match event {
                            RendererEvent::Load => Event::Load,
                            RendererEvent::ChildLoad => Event::ChildLoad,
                            RendererEvent::Navigated => Event::Navigated,
                            RendererEvent::NavigationFailed => Event::NavigationFailed,
                            RendererEvent::Timer(id) => Event::Timer(id),
                            RendererEvent::Fetch { status } => Event::Fetch(status),
                            RendererEvent::FetchFailed => Event::FetchFailed,
                            RendererEvent::ScriptFailed => Event::ScriptFailed,
                        },
                    })
                    .collect()
            })
            .map_err(|error| error.to_string())
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
        operation(&mut self.renderer.borrow_mut()).map_err(|error| error.to_string())
    }
}

struct OfflineHost;

impl BrowserServices for OfflineHost {
    fn start_dial(&self, _request: DialRequest, completion: DialCompletion) {
        completion(Err(DialFailure::Connect));
    }

    fn cookies_for(&self, _url: &Url) -> String {
        String::new()
    }

    fn set_cookie(&self, _value: &str, _url: &Url) {}
}
