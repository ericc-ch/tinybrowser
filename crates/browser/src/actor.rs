//! One OS thread per page, one long-lived current-thread Tokio runtime.
//!
//! [ADR 0010](../../../docs/adrs/0010-page-actor-ownership.md): commands, events,
//! request IDs, values, and explicit errors may cross. DOM references, `QuickJS`
//! values, callbacks, and closures must not.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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

    /// Queues navigation. Call [`PageHandle::run_until_load`] to wait.
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

    /// Parks the page actor until no jobs remain.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run(&self) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::Run { reply })?;
        recv_unit(&rx)
    }

    /// Parks until the current navigation has fired `load`.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run_until_load(&self) -> Result<(), PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoad { reply })?;
        recv_unit(&rx)
    }

    /// Parks like [`PageHandle::run_until_load`], returning `false` on timeout.
    ///
    /// # Errors
    ///
    /// [`PageError::ActorStopped`].
    pub fn run_until_load_timeout(&self, timeout: Duration) -> Result<bool, PageError> {
        let (reply, rx) = mpsc::channel();
        self.send(Command::RunUntilLoadTimeout { timeout, reply })?;
        recv_bool(&rx)
    }

    /// Parks until `source` evaluates to JS `true`, returning `false` on timeout.
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
    while let Ok(envelope) = rx.recv() {
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
                page.run();
                let _ = reply.send(());
            }
            Command::RunUntilLoad { reply } => {
                page.run_until_load();
                let _ = reply.send(());
            }
            Command::RunUntilLoadTimeout { timeout, reply } => {
                let _ = reply.send(page.run_until_load_timeout(timeout));
            }
            Command::RunUntilJsTrue {
                source,
                timeout,
                reply,
            } => {
                let _ = reply.send(page.run_until_js_true(&source, timeout));
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
            Command::LastNavigationFailed { reply } => {
                let _ = reply.send(page.last_navigation_failed());
            }
            Command::Shutdown { reply } => {
                page.shutdown_runtime();
                let _ = reply.send(());
                return;
            }
        }
    }
    page.shutdown_runtime();
}
