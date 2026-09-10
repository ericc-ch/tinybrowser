//! One OS thread per page, one long-lived current-thread Tokio runtime.
//!
//! [ADR 0010](../../../docs/adrs/0010-page-actor-ownership.md): commands, events,
//! request IDs, values, and explicit errors may cross. DOM references, `QuickJS`
//! values, callbacks, and closures must not.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::network::FetchHandle;
use crate::page::{Page, PageError, PageEvent, Stop};
use crate::remote::RemoteValue;

/// Identity of one tab in a [`crate::Browser`] registry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct PageId(u64);

impl PageId {
    /// Constructs a page id from a protocol integer.
    #[must_use]
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Stable numeric identity for protocol adapters.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Correlates one command with its reply.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct RequestId(u64);

impl RequestId {
    /// Numeric identity of this request.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

enum Command {
    LoadHtml {
        html: String,
        reply: Sender<Result<(), PageError>>,
    },
    Goto {
        url: String,
        reply: Sender<Result<(), PageError>>,
    },
    Eval {
        source: String,
        reply: Sender<Result<String, PageError>>,
    },
    Execute {
        source: String,
        timeout: Option<Duration>,
        reply: Sender<Result<RemoteValue, PageError>>,
    },
    Run {
        reply: Sender<()>,
    },
    RunUntilLoad {
        reply: Sender<()>,
    },
    RunUntilLoadTimeout {
        timeout: Duration,
        reply: Sender<bool>,
    },
    RunUntilJsTrue {
        source: String,
        timeout: Duration,
        reply: Sender<bool>,
    },
    DocumentUrl {
        reply: Sender<String>,
    },
    ContentLanguage {
        reply: Sender<Option<String>>,
    },
    CookieGet {
        reply: Sender<String>,
    },
    CookieSet {
        value: String,
        reply: Sender<()>,
    },
    SetDocumentUrl {
        url: String,
        reply: Sender<Result<(), PageError>>,
    },
    Events {
        reply: Sender<Vec<PageEvent>>,
    },
    Subscribe {
        reply: Sender<Receiver<PageEvent>>,
    },
    LastNavigationFailed {
        reply: Sender<bool>,
    },
    Shutdown {
        reply: Sender<()>,
    },
}

struct Envelope {
    request_id: RequestId,
    command: Command,
}

enum Waiter {
    Idle(Sender<()>),
    Load(Sender<()>),
    LoadTimeout {
        deadline: Instant,
        reply: Sender<bool>,
    },
    JsTrue {
        source: String,
        deadline: Instant,
        reply: Sender<bool>,
    },
}

/// Value-only handle to one [`PageActor`].
#[derive(Clone)]
pub struct PageHandle {
    id: PageId,
    next_request: Arc<AtomicU64>,
    stop: Arc<Stop>,
    tx: Sender<Envelope>,
}

impl PageHandle {
    /// Tab identity in the owning browser.
    #[must_use]
    pub fn id(&self) -> PageId {
        self.id
    }

    /// Next request id that a command from this handle would use.
    #[must_use]
    pub fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request.load(Ordering::Relaxed))
    }

    /// Parses `html` into this page and starts a new JS realm.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`] when the actor has shut down.
    pub fn load_html(&self, html: &str) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::LoadHtml {
            html: html.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Starts navigation. The page continues independently; call
    /// [`PageHandle::run_until_load`] only when the caller needs to wait.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] or [`PageError::ActorStopped`].
    pub fn goto(&self, url: &str) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Goto {
            url: url.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Evaluates `source` and returns its string coercion.
    ///
    /// # Errors
    ///
    /// [`PageError::Script`] or [`PageError::ActorStopped`].
    pub fn eval(&self, source: &str) -> Result<String, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Eval {
            source: source.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Evaluates `source` and returns a value-only script result.
    ///
    /// # Errors
    ///
    /// [`PageError::Script`] or [`PageError::ActorStopped`].
    pub fn execute_script(&self, source: &str) -> Result<RemoteValue, PageError> {
        self.execute_script_timeout(source, None)
    }

    /// Evaluates `source`, interrupting `QuickJS` if `timeout` elapses.
    ///
    /// # Errors
    ///
    /// [`PageError::Script`] or [`PageError::ActorStopped`].
    pub fn execute_script_timeout(
        &self,
        source: &str,
        timeout: Option<Duration>,
    ) -> Result<RemoteValue, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Execute {
            source: source.to_owned(),
            timeout,
            reply,
        })?;
        recv_result(&rx)
    }

    /// Waits until no page jobs remain without preventing other commands.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run(&self) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Run { reply })?;
        recv_unit(&rx)
    }

    /// Waits until the current navigation has fired `load`.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run_until_load(&self) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoad { reply })?;
        recv_unit(&rx)
    }

    /// Waits like [`PageHandle::run_until_load`], returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoadTimeout { timeout, reply })?;
        recv_bool(&rx)
    }

    /// Waits until `source` evaluates to JS `true`, returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run_until_js_true(&self, source: &str, timeout: Duration) -> Result<bool, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilJsTrue {
            source: source.to_owned(),
            timeout,
            reply,
        })?;
        recv_bool(&rx)
    }

    /// Document URL after navigation.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn document_url(&self) -> Result<String, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::DocumentUrl { reply })?;
        recv_text(&rx)
    }

    /// Document `Content-Language`, if any.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn content_language(&self) -> Result<Option<String>, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::ContentLanguage { reply })?;
        recv_optional_text(&rx)
    }

    /// `document.cookie` getter.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn document_cookie(&self) -> Result<String, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::CookieGet { reply })?;
        recv_text(&rx)
    }

    /// `document.cookie` setter.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn set_document_cookie(&self, value: &str) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::CookieSet {
            value: value.to_owned(),
            reply,
        })?;
        recv_unit(&rx)
    }

    /// Sets the document URL used as cookie initiator and relative-URL base.
    ///
    /// # Errors
    ///
    /// [`PageError::InvalidUrl`] or [`PageError::ActorStopped`].
    pub fn set_document_url(&self, url: &str) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::SetDocumentUrl {
            url: url.to_owned(),
            reply,
        })?;
        recv_result(&rx)
    }

    /// Jobs that have already run, in order.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn events(&self) -> Result<Vec<PageEvent>, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Events { reply })?;
        recv_events(&rx)
    }

    /// Subscribes to page events emitted after this call.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`] when the actor has shut down.
    pub fn subscribe(&self) -> Result<Receiver<PageEvent>, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Subscribe { reply })?;
        rx.recv().map_err(|_| PageError::ActorStopped)
    }

    /// True when the last navigation dial failed.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn last_navigation_failed(&self) -> Result<bool, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::LastNavigationFailed { reply })?;
        recv_bool(&rx)
    }

    /// Asks the actor to stop. Further commands fail with [`PageError::ActorStopped`].
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`] if the actor is already gone.
    pub fn shutdown(&self) -> Result<(), PageError> {
        self.stop.request();
        let (reply, rx) = mpsc::channel();
        self.send(Command::Shutdown { reply })?;
        recv_unit(&rx)
    }

    fn send(&self, command: Command) -> Result<(), PageError> {
        let request_id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        self.tx
            .send(Envelope {
                request_id,
                command,
            })
            .map_err(|_| PageError::ActorStopped)
    }
}

fn recv_result<T>(rx: &Receiver<Result<T, PageError>>) -> Result<T, PageError> {
    rx.recv().unwrap_or(Err(PageError::ActorStopped))
}

fn recv_unit(rx: &Receiver<()>) -> Result<(), PageError> {
    rx.recv().map_err(|_| PageError::ActorStopped)
}

fn recv_bool(rx: &Receiver<bool>) -> Result<bool, PageError> {
    rx.recv().map_err(|_| PageError::ActorStopped)
}

fn recv_text(rx: &Receiver<String>) -> Result<String, PageError> {
    rx.recv().map_err(|_| PageError::ActorStopped)
}

fn recv_optional_text(rx: &Receiver<Option<String>>) -> Result<Option<String>, PageError> {
    rx.recv().map_err(|_| PageError::ActorStopped)
}

fn recv_events(rx: &Receiver<Vec<PageEvent>>) -> Result<Vec<PageEvent>, PageError> {
    rx.recv().map_err(|_| PageError::ActorStopped)
}

/// Join handle and command sender for one page thread.
pub(crate) struct PageActor {
    pub handle: PageHandle,
    join: Option<JoinHandle<()>>,
}

impl PageActor {
    pub(crate) fn spawn(id: PageId, fetch: FetchHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(Stop::new());
        let handle = PageHandle {
            id,
            next_request: Arc::new(AtomicU64::new(1)),
            stop: Arc::clone(&stop),
            tx,
        };
        let join = thread::Builder::new()
            .name(format!("page-{id}"))
            .spawn(move || actor_loop(&rx, fetch, stop))
            .expect("page actor thread");
        Self {
            handle,
            join: Some(join),
        }
    }

    pub(crate) fn shutdown(&mut self) {
        self.handle.stop.request();
        let (reply, rx) = mpsc::channel();
        let _ = self.handle.tx.send(Envelope {
            request_id: RequestId(0),
            command: Command::Shutdown { reply },
        });
        let _ = rx.recv_timeout(Duration::from_secs(2));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PageActor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn actor_loop(rx: &Receiver<Envelope>, fetch: FetchHandle, stop: Arc<Stop>) {
    let mut page = Page::with_fetch_stop(fetch, stop);
    let mut subscribers = Vec::new();
    let mut published_events = 0;
    let mut waiters = Vec::new();
    loop {
        let received = if page.has_background_work() || !waiters.is_empty() {
            rx.recv_timeout(Duration::from_millis(10))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let envelope = match received {
            Ok(envelope) => envelope,
            Err(RecvTimeoutError::Timeout) => {
                page.drive_for(Duration::from_millis(10));
                publish_events(&page, &mut published_events, &mut subscribers);
                resolve_waiters(&mut page, &mut waiters);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let _request_id = envelope.request_id;
        match envelope.command {
            Command::LoadHtml { html, reply } => {
                page.load_html(&html);
                let _ = reply.send(Ok(()));
            }
            Command::Goto { url, reply } => {
                let _ = reply.send(page.goto(&url));
            }
            Command::Eval { source, reply } => {
                let _ = reply.send(page.eval(&source));
            }
            Command::Execute {
                source,
                timeout,
                reply,
            } => {
                let _ = reply.send(page.execute_remote(&source, timeout));
            }
            Command::Run { reply } => {
                waiters.push(Waiter::Idle(reply));
            }
            Command::RunUntilLoad { reply } => {
                waiters.push(Waiter::Load(reply));
            }
            Command::RunUntilLoadTimeout { timeout, reply } => {
                waiters.push(Waiter::LoadTimeout {
                    deadline: Instant::now() + timeout,
                    reply,
                });
            }
            Command::RunUntilJsTrue {
                source,
                timeout,
                reply,
            } => {
                waiters.push(Waiter::JsTrue {
                    source,
                    deadline: Instant::now() + timeout,
                    reply,
                });
            }
            Command::DocumentUrl { reply } => {
                let _ = reply.send(page.document_url().to_owned());
            }
            Command::ContentLanguage { reply } => {
                let _ = reply.send(page.content_language().map(str::to_owned));
            }
            Command::CookieGet { reply } => {
                let _ = reply.send(page.document_cookie());
            }
            Command::CookieSet { value, reply } => {
                page.set_document_cookie(&value);
                let _ = reply.send(());
            }
            Command::SetDocumentUrl { url, reply } => {
                let _ = reply.send(page.set_document_url(&url));
            }
            Command::Events { reply } => {
                let _ = reply.send(page.events().to_vec());
            }
            Command::Subscribe { reply } => {
                let (events, event_rx) = mpsc::channel();
                subscribers.push(events);
                let _ = reply.send(event_rx);
            }
            Command::LastNavigationFailed { reply } => {
                let _ = reply.send(page.last_navigation_failed());
            }
            Command::Shutdown { reply } => {
                page.shutdown_runtime();
                let _ = reply.send(());
                return;
            }
        }
        publish_events(&page, &mut published_events, &mut subscribers);
        resolve_waiters(&mut page, &mut waiters);
    }
    page.shutdown_runtime();
}

fn resolve_waiters(page: &mut Page, waiters: &mut Vec<Waiter>) {
    let now = Instant::now();
    let mut pending = Vec::new();
    for waiter in std::mem::take(waiters) {
        match waiter {
            Waiter::Idle(reply) if !page.has_background_work() => {
                let _ = reply.send(());
            }
            Waiter::Load(reply) if !page.waiting_for_load() => {
                let _ = reply.send(());
            }
            Waiter::LoadTimeout { reply, .. } if !page.waiting_for_load() => {
                let _ = reply.send(true);
            }
            Waiter::LoadTimeout { deadline, reply } if now >= deadline => {
                let _ = reply.send(false);
            }
            Waiter::JsTrue {
                source,
                deadline,
                reply,
            } => {
                if now >= deadline {
                    let _ = reply.send(false);
                } else if matches!(
                    page.execute_script(&source),
                    Ok(crate::ScriptValue::Bool(true))
                ) {
                    let _ = reply.send(true);
                } else {
                    pending.push(Waiter::JsTrue {
                        source,
                        deadline,
                        reply,
                    });
                }
            }
            other => pending.push(other),
        }
    }
    *waiters = pending;
}

fn publish_events(page: &Page, cursor: &mut usize, subscribers: &mut Vec<Sender<PageEvent>>) {
    let events = &page.events()[*cursor..];
    subscribers.retain(|subscriber| events.iter().all(|event| subscriber.send(*event).is_ok()));
    *cursor = page.events().len();
}
