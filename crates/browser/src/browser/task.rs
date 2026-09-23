use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::actor::{TabHandle, TabId, TabTask};
use crate::context::BrowserContext;
use crate::exchange::ServerInput;
use crate::manager::{RendererProcessManager, RendererProcessManagerOptions};

use super::{
    AddCookie, BrowserError, BrowserHandle, BrowserOperation, BrowserServer, ClearCookies,
    CloseTab, Command, CookieRecords, CreateTab, GetTab, ListTabs, Notice, OpenWindowOptions,
    Reply,
};

/// The browser's tab table, shared with renderer service tasks.
///
/// Service tasks resolve cross-tab calls through this registry, so the browser
/// owner task never waits on a tab coordinator and the wait-for graph stays
/// acyclic. `postMessage` enqueues on the target tab's delivery mailbox and
/// returns; a busy target cannot block the sender's renderer.
#[derive(Clone)]
pub(crate) struct TabRegistry {
    tabs: Arc<Mutex<HashMap<TabId, TabTask>>>,
    openers: Arc<Mutex<HashMap<TabId, TabId>>>,
}

impl TabRegistry {
    fn new() -> Self {
        Self {
            tabs: Arc::new(Mutex::new(HashMap::new())),
            openers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The handle of one live tab.
    pub(crate) fn tab(&self, id: TabId) -> Option<TabHandle> {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map(|task| task.handle.clone())
    }

    /// The opener of one live tab.
    pub(crate) fn opener(&self, id: TabId) -> Option<TabId> {
        self.openers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .copied()
    }

    /// Queues one `postMessage` payload on the target tab's mailbox. Returns
    /// false when the tab is gone or its coordinator stopped.
    pub(crate) async fn deliver(&self, id: TabId, payload: String) -> bool {
        let sender = self
            .tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map(TabTask::deliveries);
        match sender {
            Some(sender) => {
                let queued = sender.send(payload).await.is_ok();
                if queued {
                    logging::debug!(
                        target: "browser::tabs",
                        "window message queued for tab {id}"
                    );
                }
                queued
            }
            None => false,
        }
    }

    fn ids(&self) -> Vec<TabId> {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect()
    }

    fn insert(&self, id: TabId, task: TabTask) {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, task);
    }

    fn remove(&self, id: TabId) -> Option<TabTask> {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    fn drain(&self) -> Vec<TabTask> {
        self.tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, task)| task)
            .collect()
    }

    fn set_opener(&self, id: TabId, opener: TabId) {
        self.openers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, opener);
    }

    fn remove_opener(&self, id: TabId) {
        self.openers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

pub(crate) struct BrowserTask {
    server: BrowserServer,
    context: BrowserContext,
    renderers: Arc<RendererProcessManager>,
    tabs: TabRegistry,
    next_tab: Option<u64>,
}

/// Dependencies one [`BrowserTask`] starts with.
pub(super) struct BrowserTaskOptions {
    /// Server end of the browser exchange.
    pub(super) server: BrowserServer,
    /// Live profile context.
    pub(super) context: BrowserContext,
    /// Handle cloned into the renderer manager for `window.open` callbacks.
    pub(super) browser: BrowserHandle,
}

enum DispatchOutcome {
    Continue(Reply),
    Stop(Reply),
}

impl BrowserTask {
    pub(super) fn new(parts: BrowserTaskOptions) -> Self {
        let BrowserTaskOptions {
            server,
            context,
            browser,
        } = parts;
        let tabs = TabRegistry::new();
        let renderers = Arc::new(RendererProcessManager::new(
            browser,
            RendererProcessManagerOptions {
                partition: context.partition_services(),
                sessions: context.session_storage(),
                tabs: tabs.clone(),
            },
        ));
        Self {
            server,
            context,
            renderers,
            tabs,
            next_tab: Some(1),
        }
    }

    pub(super) async fn run(mut self) {
        loop {
            let Some(input) = self.server.recv().await else {
                break;
            };
            match input {
                ServerInput::Call { id, body } => match self.dispatch(body).await {
                    DispatchOutcome::Continue(reply) => {
                        if self.server.reply(id, reply).await.is_err() {
                            break;
                        }
                    }
                    DispatchOutcome::Stop(reply) => {
                        let _result = self.server.reply(id, reply).await;
                        return;
                    }
                },
                ServerInput::Notify(Notice::Close) => break,
                ServerInput::RequestChunk { .. }
                | ServerInput::RequestEnd { .. }
                | ServerInput::Cancel { .. } => {}
            }
        }
        let _result = self.shutdown().await;
    }

    async fn dispatch(&mut self, command: Command) -> DispatchOutcome {
        let reply = match command {
            Command::CreateTab => CreateTab.serve(self).await,
            Command::Tabs => ListTabs.serve(self).await,
            Command::Tab { id } => GetTab { id }.serve(self).await,
            Command::CloseTab { id } => CloseTab { id }.serve(self).await,
            Command::CookieRecords { url } => CookieRecords { url }.serve(self).await,
            Command::ClearCookies => ClearCookies.serve(self).await,
            Command::AddCookie { cookie, url } => AddCookie { cookie, url }.serve(self).await,
            Command::OpenWindow(request) => request.serve(self).await,
            Command::Close => {
                let result = self.shutdown().await;
                return DispatchOutcome::Stop(Reply::Close(result));
            }
        };
        DispatchOutcome::Continue(reply)
    }

    fn create_tab(&mut self) -> Result<TabHandle, BrowserError> {
        let raw = self.next_tab.ok_or(BrowserError::TabIdExhausted)?;
        self.next_tab = raw.checked_add(1);
        let id = TabId::new(raw);
        self.context.create_tab(id);
        let task = TabTask::spawn(crate::actor::SpawnOptions {
            id,
            network: self.context.tab_network(),
            renderers: Arc::clone(&self.renderers),
        });
        let handle = task.handle.clone();
        self.tabs.insert(id, task);
        Ok(handle)
    }

    fn close_tab(&mut self, id: TabId) -> Result<(), BrowserError> {
        let Some(mut task) = self.tabs.remove(id) else {
            return Err(BrowserError::UnknownTab);
        };
        self.tabs.remove_opener(id);
        // Never wait on the tab coordinator here: the browser owner task must
        // not join a wait-for cycle with a renderer that needs it. The task
        // keeps its own lifecycle and reaps itself.
        tokio::spawn(async move { task.shutdown().await });
        self.context.close_tab(id);
        Ok(())
    }

    fn open_window(&mut self, request: OpenWindowOptions) -> Result<TabId, BrowserError> {
        let OpenWindowOptions {
            url,
            source,
            noopener,
        } = request;
        let handle = self.create_tab()?;
        let id = handle.id();
        if let (Some(source), false) = (source, noopener) {
            self.tabs.set_opener(id, source);
            self.context.copy_session(source, id);
        }
        if !url.is_empty() && url != "about:blank" {
            tokio::spawn(async move {
                let _result = handle.goto(&url).await;
            });
        }
        Ok(id)
    }

    async fn shutdown(&mut self) -> std::io::Result<()> {
        for mut task in self.tabs.drain() {
            task.shutdown().await;
        }
        self.context.persist().await
    }
}

impl BrowserOperation for CreateTab {
    type Output = Result<TabHandle, BrowserError>;

    fn into_command(self) -> Command {
        Command::CreateTab
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::CreateTab(result) => Some(result),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::CreateTab(task.create_tab())
    }
}

impl BrowserOperation for ListTabs {
    type Output = Vec<TabId>;

    fn into_command(self) -> Command {
        Command::Tabs
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::Tabs(tabs) => Some(tabs),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::Tabs(task.tabs.ids())
    }
}

impl BrowserOperation for GetTab {
    type Output = Result<TabHandle, BrowserError>;

    fn into_command(self) -> Command {
        Command::Tab { id: self.id }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::Tab(result) => Some(result),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::Tab(task.tabs.tab(self.id).ok_or(BrowserError::UnknownTab))
    }
}

impl BrowserOperation for CloseTab {
    type Output = Result<(), BrowserError>;

    fn into_command(self) -> Command {
        Command::CloseTab { id: self.id }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::CloseTab(result) => Some(result),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::CloseTab(task.close_tab(self.id))
    }
}

impl BrowserOperation for CookieRecords {
    type Output = Vec<net::CookieRecord>;

    fn into_command(self) -> Command {
        Command::CookieRecords { url: self.url }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::CookieRecords(records) => Some(records),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::CookieRecords(task.context.cookie_records(&self.url))
    }
}

impl BrowserOperation for ClearCookies {
    type Output = ();

    fn into_command(self) -> Command {
        Command::ClearCookies
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::ClearCookies => Some(()),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        task.context.clear_cookies();
        Reply::ClearCookies
    }
}

impl BrowserOperation for AddCookie {
    type Output = bool;

    fn into_command(self) -> Command {
        Command::AddCookie {
            cookie: self.cookie,
            url: self.url,
        }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::AddCookie(stored) => Some(stored),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::AddCookie(task.context.add_cookie(&self.cookie, &self.url))
    }
}

impl BrowserOperation for OpenWindowOptions {
    type Output = Result<TabId, BrowserError>;

    fn into_command(self) -> Command {
        Command::OpenWindow(self)
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::OpenWindow(result) => Some(result),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::OpenWindow(task.open_window(self))
    }
}
