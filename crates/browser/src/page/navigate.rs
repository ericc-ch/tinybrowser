use std::collections::HashSet;

use net::{Context, Method};
use url::Url;

use crate::js::ClassicScript;
use crate::network::FetchHandle;
use crate::parse_html;

use super::{CompletedDial, DialFail, FETCH_BODY_LIMIT, Page, PageError, PageEvent, QueuedDial};

impl Page {
    /// Parses `input` into this page's tree and starts a new JS realm.
    pub fn load_html(&mut self, input: &str) {
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::Navigate { .. }));
        self.reset_js_realm();
        {
            let mut world = self.world.borrow_mut();
            world.replace_document(parse_html(input));
            if let Some(parsed) = world.parsed.as_mut() {
                parsed
                    .dom
                    .set_document_language(self.content_language.clone());
            }
        }
        self.boot_document();
        self.fetch.persist();
    }

    /// Queues a GET `fetch` job. [`Page::run`] performs the send.
    /// Relative URLs resolve against `<base href>` or the document URL.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] when `url` cannot be parsed or joined, or
    /// is not `http`/`https`.
    pub fn start_fetch(&mut self, url: &str) -> Result<(), PageError> {
        let url = self.resolve_dial_url(url)?;
        let initiator = self.document_url.clone();
        self.queued_dials.push(QueuedDial::Fetch { url, initiator });
        Ok(())
    }

    /// Queues a navigation GET with [`Context::Navigation`]. [`Page::run`]
    /// performs the send, parses the body as HTML, stores `Content-Language`,
    /// and starts a new JS realm.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] when `url` cannot be parsed or joined, or
    /// is not `http`/`https`.
    pub fn goto(&mut self, url: &str) -> Result<(), PageError> {
        let url = self.resolve_dial_url(url)?;
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.navigation_failed = false;
        self.nav_in_flight = None;
        let epoch = self.nav_epoch;
        let initiator = self.document_url.clone();
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::Navigate { .. }));
        self.queued_dials.push(QueuedDial::Navigate {
            url,
            initiator,
            epoch,
        });
        Ok(())
    }

    pub(in crate::page) fn resolve_dial_url(&self, spec: &str) -> Result<Url, PageError> {
        let url = Url::parse(spec)
            .or_else(|_| self.base_url().join(spec))
            .map_err(|_| PageError::InvalidUrl { spec: spec.into() })?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(PageError::InvalidUrl { spec: spec.into() });
        }
        Ok(url)
    }

    fn base_url(&self) -> Url {
        let world = self.world.borrow();
        let Some(parsed) = world.parsed.as_ref() else {
            return self.document_url.clone();
        };
        let Ok(Some(base_el)) = parsed.dom.select_first(parsed.dom.document(), "base[href]") else {
            return self.document_url.clone();
        };
        let Some(href) = parsed.dom.attribute(base_el, "href") else {
            return self.document_url.clone();
        };
        self.document_url
            .join(&href)
            .unwrap_or_else(|_| self.document_url.clone())
    }

    pub(in crate::page) fn finish_dial(&mut self, done: CompletedDial) {
        match done {
            CompletedDial::Fetch { status } => {
                self.events.push(PageEvent::Fetch { status });
            }
            CompletedDial::Navigate {
                status,
                body,
                final_url,
                content_language,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if self.nav_in_flight == Some(epoch) {
                    self.nav_in_flight = None;
                }
                if epoch == self.nav_epoch {
                    self.navigation_failed = false;
                    self.apply_navigation(final_url, content_language, &body);
                }
            }
            CompletedDial::JsFetch {
                status,
                body,
                id,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if epoch == self.js_epoch {
                    let body = String::from_utf8_lossy(&body);
                    self.settle_js_fetch(id, true, i32::from(status), &body);
                }
            }
            CompletedDial::ClassicScript {
                status,
                body,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.pending_classic.pop_front();
                    if (200..300).contains(&status) {
                        let source = String::from_utf8_lossy(&body);
                        self.eval_classic(&source);
                    }
                    self.advance_classic_scripts();
                }
            }
        }
        self.adopt_js_work();
    }

    pub(in crate::page) fn fail_dial(&mut self, fail: DialFail) {
        self.events.push(PageEvent::FetchFailed);
        match fail {
            DialFail::Fetch => {}
            DialFail::Navigate { epoch } => {
                if self.nav_in_flight == Some(epoch) {
                    self.nav_in_flight = None;
                }
                if epoch == self.nav_epoch {
                    self.navigation_failed = true;
                }
            }
            DialFail::JsFetch { id, epoch } => {
                if epoch == self.js_epoch {
                    self.settle_js_fetch(id, false, 0, "");
                }
            }
            DialFail::ClassicScript { epoch } => {
                if epoch == self.js_epoch {
                    self.classic_fetch_in_flight = false;
                    self.pending_classic.pop_front();
                    self.advance_classic_scripts();
                }
            }
        }
        self.adopt_js_work();
    }

    fn apply_navigation(&mut self, final_url: Url, content_language: Option<String>, body: &[u8]) {
        self.reset_js_realm();
        let html = String::from_utf8_lossy(body);
        self.document_url = final_url.clone();
        self.content_language.clone_from(&content_language);
        {
            let mut world = self.world.borrow_mut();
            world.document_url = final_url;
            world.replace_document(parse_html(&html));
            if let Some(parsed) = world.parsed.as_mut() {
                parsed.dom.set_document_language(content_language);
            }
        }
        self.boot_document();
    }

    pub(in crate::page) fn settle_js_fetch(&mut self, id: i32, ok: bool, status: i32, body: &str) {
        if let Some(js) = &self.js {
            super::note_script(
                &mut self.events,
                js.finish_js_fetch(id, ok, status, body).is_err(),
            );
        }
    }

    fn reset_js_realm(&mut self) {
        self.js_epoch = self.js_epoch.saturating_add(1);
        self.queued_dials.retain(|dial| {
            !matches!(
                dial,
                QueuedDial::JsFetch { .. } | QueuedDial::ClassicScript { .. }
            )
        });
        let js_timer_ids: HashSet<u32> = self.js_timer_slots.keys().copied().collect();
        self.timers
            .retain(|timer| !js_timer_ids.contains(&timer.id));
        self.js_timer_slots.clear();
        self.js = None;
        self.pending_classic.clear();
        self.classic_fetch_in_flight = false;
        self.remote_by_node.clear();
    }

    fn boot_document(&mut self) {
        if self.ensure_js().is_err() {
            self.events.push(PageEvent::ScriptFailed);
            return;
        }
        let scripts = crate::js::collect_classic_scripts(&self.world.borrow());
        self.pending_classic = scripts.into();
        self.advance_classic_scripts();
    }

    fn eval_classic(&mut self, source: &str) {
        if let Some(js) = &self.js {
            super::note_script(&mut self.events, js.eval(source).is_err());
        }
        self.adopt_js_work();
    }

    fn advance_classic_scripts(&mut self) {
        loop {
            match self.pending_classic.front().cloned() {
                Some(ClassicScript::Inline(source)) => {
                    self.pending_classic.pop_front();
                    self.eval_classic(&source);
                }
                Some(ClassicScript::Src(src)) => {
                    if self.classic_fetch_in_flight {
                        return;
                    }
                    if let Ok(url) = self.resolve_dial_url(&src) {
                        self.classic_fetch_in_flight = true;
                        let initiator = self.document_url.clone();
                        self.queued_dials.push(QueuedDial::ClassicScript {
                            url,
                            initiator,
                            epoch: self.js_epoch,
                        });
                    } else {
                        self.pending_classic.pop_front();
                        continue;
                    }
                    return;
                }
                None => {
                    self.fire_document_load();
                    return;
                }
            }
        }
    }

    fn fire_document_load(&mut self) {
        if self.world.borrow().document_ready {
            return;
        }
        self.world.borrow_mut().document_ready = true;
        if let Some(js) = &self.js {
            super::note_script(&mut self.events, js.fire_load().is_err());
        }
        self.adopt_js_work();
    }
}

pub(in crate::page) fn send_dial(
    fetch: &FetchHandle,
    dial: &QueuedDial,
) -> Result<CompletedDial, DialFail> {
    let fail = match dial {
        QueuedDial::Fetch { .. } => DialFail::Fetch,
        QueuedDial::Navigate { epoch, .. } => DialFail::Navigate { epoch: *epoch },
        QueuedDial::JsFetch { id, epoch, .. } => DialFail::JsFetch {
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => DialFail::ClassicScript { epoch: *epoch },
    };
    let (url, context, initiator, read_body) = match dial {
        QueuedDial::Fetch { url, initiator } => (url, Context::Fetch, initiator, false),
        QueuedDial::Navigate { url, initiator, .. } => (url, Context::Navigation, initiator, true),
        QueuedDial::JsFetch { url, initiator, .. }
        | QueuedDial::ClassicScript { url, initiator, .. } => {
            (url, Context::Fetch, initiator, true)
        }
    };
    let response = fetch
        .request(Method::GET, url.clone())
        .with_context(context)
        .with_initiator(initiator.clone())
        .send()
        .map_err(|_| fail)?;
    fetch.mark_dirty();
    if matches!(dial, QueuedDial::Navigate { .. }) {
        fetch.persist();
    }
    let status = response.status();
    let final_url = response.final_url().clone();
    let content_language = response
        .headers()
        .get("content-language")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(content_language_tag);
    let body = if read_body {
        response
            .into_body()
            .bytes(FETCH_BODY_LIMIT)
            .map_err(|_| fail)?
    } else {
        Vec::new()
    };
    Ok(match dial {
        QueuedDial::Fetch { .. } => CompletedDial::Fetch { status },
        QueuedDial::Navigate { epoch, .. } => CompletedDial::Navigate {
            status,
            body,
            final_url,
            content_language,
            epoch: *epoch,
        },
        QueuedDial::JsFetch { id, epoch, .. } => CompletedDial::JsFetch {
            status,
            body,
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => CompletedDial::ClassicScript {
            status,
            body,
            epoch: *epoch,
        },
    })
}

/// One `Content-Language` tag, or `None` when the header lists several
/// languages ([HTML document language](https://html.spec.whatwg.org/multipage/dom.html#language)).
fn content_language_tag(raw: &str) -> Option<String> {
    let mut tags = raw
        .split(',')
        .map(|part| part.split(';').next().unwrap_or(part).trim())
        .filter(|tag| !tag.is_empty());
    let first = tags.next()?.to_owned();
    if tags.next().is_some() {
        return None;
    }
    Some(first)
}
