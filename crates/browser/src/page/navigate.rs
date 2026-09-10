use std::collections::HashSet;

use net::{Context, Method};
use url::Url;

use crate::js::ClassicScript;
use crate::network::FetchHandle;
use crate::{ActiveParser, ParseProgress};

use super::{
    CompletedDial, DialFail, FETCH_BODY_LIMIT, Page, PageError, PageEvent, QueuedDial, Stop,
};

impl Page {
    /// Parses `input` into this page's tree and starts a new JS realm.
    pub fn load_html(&mut self, input: &str) {
        self.nav_epoch = self.nav_epoch.saturating_add(1);
        self.queued_dials
            .retain(|dial| !matches!(dial, QueuedDial::Navigate { .. }));
        self.reset_js_realm();
        self.start_document(input);
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
                content_type,
                epoch,
            } => {
                self.events.push(PageEvent::Fetch { status });
                if self.nav_in_flight == Some(epoch) {
                    self.nav_in_flight = None;
                }
                if epoch == self.nav_epoch {
                    self.navigation_failed = false;
                    self.apply_navigation(
                        final_url,
                        content_language,
                        content_type.as_deref(),
                        &body,
                    );
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
                    if (200..300).contains(&status) {
                        let source = String::from_utf8_lossy(&body);
                        self.eval_classic(&source);
                    }
                    self.sync_parser_from_world();
                    self.advance_parser();
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
                    self.sync_parser_from_world();
                    self.advance_parser();
                }
            }
        }
        self.adopt_js_work();
    }

    fn apply_navigation(
        &mut self,
        final_url: Url,
        content_language: Option<String>,
        content_type: Option<&str>,
        body: &[u8],
    ) {
        self.reset_js_realm();
        let html = decode_html(body, content_type);
        self.document_url = final_url.clone();
        self.content_language = content_language;
        self.world.borrow_mut().document_url = final_url;
        self.start_document(&html);
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
        self.active_parser = None;
        self.world.borrow_mut().parser_active = false;
        self.world.borrow_mut().pending_html_writes.clear();
        self.classic_fetch_in_flight = false;
        self.remote_by_node.clear();
    }

    fn start_document(&mut self, html: &str) {
        self.world.borrow_mut().parser_active = true;
        self.active_parser = Some(ActiveParser::new(html));
        self.advance_parser();
    }

    fn eval_classic(&mut self, source: &str) {
        if let Some(js) = &self.js {
            super::note_script(&mut self.events, js.eval(source).is_err());
        }
        self.adopt_js_work();
    }

    // https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-intext
    // https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element
    fn advance_parser(&mut self) {
        loop {
            let Some(parser) = self.active_parser.as_ref() else {
                return;
            };
            match parser.advance() {
                ParseProgress::Script(id) => {
                    let parsed = parser.take_state();
                    if self.js.is_none() {
                        self.world.borrow_mut().replace_document(parsed);
                        if self.ensure_js().is_err() {
                            self.events.push(PageEvent::ScriptFailed);
                            self.sync_parser_from_world();
                            continue;
                        }
                    } else {
                        self.world.borrow_mut().parsed = Some(parsed);
                    }
                    if let Some(parsed) = self.world.borrow_mut().parsed.as_mut() {
                        parsed
                            .dom
                            .set_document_language(self.content_language.clone());
                    }
                    let script = crate::js::classic_script_at(&self.world.borrow(), id);
                    match script {
                        Some(ClassicScript::Inline(source)) => {
                            self.eval_classic(&source);
                            self.sync_parser_from_world();
                        }
                        Some(ClassicScript::Src(src)) => {
                            if let Ok(url) = self.resolve_dial_url(&src) {
                                self.classic_fetch_in_flight = true;
                                let initiator = self.document_url.clone();
                                self.queued_dials.push(QueuedDial::ClassicScript {
                                    url,
                                    initiator,
                                    epoch: self.js_epoch,
                                });
                                return;
                            }
                            self.sync_parser_from_world();
                        }
                        None => self.sync_parser_from_world(),
                    }
                }
                ParseProgress::Done => {
                    let Some(parser) = self.active_parser.take() else {
                        return;
                    };
                    let mut parsed = parser.finish();
                    parsed
                        .dom
                        .set_document_language(self.content_language.clone());
                    if self.js.is_none() {
                        self.world.borrow_mut().replace_document(parsed);
                        if self.ensure_js().is_err() {
                            self.events.push(PageEvent::ScriptFailed);
                            return;
                        }
                    } else {
                        self.world.borrow_mut().parsed = Some(parsed);
                    }
                    self.world.borrow_mut().parser_active = false;
                    self.fire_document_load();
                    return;
                }
            }
        }
    }

    fn sync_parser_from_world(&self) {
        let Some(parser) = self.active_parser.as_ref() else {
            return;
        };
        let mut world = self.world.borrow_mut();
        let writes = std::mem::take(&mut world.pending_html_writes).concat();
        if let Some(parsed) = world.parsed.take() {
            parser.restore(parsed);
        }
        drop(world);
        if !writes.is_empty() {
            parser.insert_html(writes);
        }
    }

    fn fire_document_load(&mut self) {
        if self.world.borrow().document_ready {
            return;
        }
        self.world.borrow_mut().document_ready = true;
        self.events.push(PageEvent::Load);
        if let Some(js) = &self.js {
            super::note_script(&mut self.events, js.fire_load().is_err());
        }
        self.adopt_js_work();
    }
}

pub(in crate::page) fn send_dial(
    fetch: &FetchHandle,
    dial: &QueuedDial,
    stop: &Stop,
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
    if stop.is_set() {
        return Err(fail);
    }
    fetch.mark_dirty();
    let status = response.status();
    let final_url = response.final_url().clone();
    let content_language = response
        .headers()
        .get("content-language")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(content_language_tag);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_owned);
    let body = if read_body {
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk) = body.read_chunk().map_err(|_| fail)? {
            if stop.is_set() || bytes.len().saturating_add(chunk.len()) > FETCH_BODY_LIMIT {
                return Err(fail);
            }
            bytes.extend_from_slice(&chunk);
        }
        bytes
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
            content_type,
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

// https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm
// https://encoding.spec.whatwg.org/#concept-encoding-get
fn decode_html(body: &[u8], content_type: Option<&str>) -> String {
    let bom = encoding_rs::Encoding::for_bom(body);
    let header = content_type.and_then(charset_from_content_type);
    let prescan = prescan_charset(body);
    let (encoding, bom_len) = bom
        .or_else(|| header.map(|encoding| (encoding, 0)))
        .or_else(|| prescan.map(|encoding| (encoding, 0)))
        .unwrap_or((encoding_rs::WINDOWS_1252, 0));
    encoding
        .decode_without_bom_handling(&body[bom_len..])
        .0
        .into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<&'static encoding_rs::Encoding> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let label = value.trim().trim_matches(['\'', '"']);
        encoding_rs::Encoding::for_label(label.as_bytes())
    })
}

fn prescan_charset(body: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let prefix = &body[..body.len().min(1024)];
    let ascii = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative_start) = ascii[cursor..].find("<meta") {
        let start = cursor + relative_start;
        let end = ascii[start..]
            .find('>')
            .map_or(ascii.len(), |offset| start + offset);
        let tag = &ascii[start..end];
        if let Some(relative_charset) = tag.find("charset=") {
            let label = tag[relative_charset + "charset=".len()..]
                .trim_start_matches([' ', '\t', '\r', '\n', '\'', '"'])
                .split([' ', '\t', '\r', '\n', '\'', '"', ';', '>'])
                .next()?;
            if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                return Some(encoding);
            }
        }
        cursor = end.saturating_add(1);
    }
    None
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
