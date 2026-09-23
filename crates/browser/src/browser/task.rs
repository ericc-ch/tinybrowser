use std::collections::HashMap;
use std::sync::Arc;

use crate::actor::{TabHandle, TabId, TabTask};
use crate::context::BrowserContext;
use crate::exchange::ServerInput;
use crate::manager::{RendererProcessManager, RendererProcessManagerOptions};

use super::{
    AddCookie, BrowserError, BrowserHandle, BrowserOperation, BrowserServer, ClearCookies,
    CloseTab, Command, CookieRecords, CreateTab, GetTab, ListTabs, Notice, OpenWindowOptions,
    OpenerTab, Reply, WindowMessage,
};

pub(crate) struct BrowserTask {
    server: BrowserServer,
    context: BrowserContext,
    renderers: Arc<RendererProcessManager>,
    tabs: HashMap<TabId, TabTask>,
    next_tab: Option<u64>,
    openers: HashMap<TabId, TabId>,
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
        let renderers = Arc::new(RendererProcessManager::new(
            browser,
            RendererProcessManagerOptions {
                partition: context.partition_services(),
                sessions: context.session_storage(),
            },
        ));
        Self {
            server,
            context,
            renderers,
            tabs: HashMap::new(),
            next_tab: Some(1),
            openers: HashMap::new(),
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
            Command::OpenerTab { tab } => OpenerTab { tab }.serve(self).await,
            Command::WindowMessage { target, payload } => {
                WindowMessage { target, payload }.serve(self).await
            }
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

    async fn close_tab(&mut self, id: TabId) -> Result<(), BrowserError> {
        let Some(mut task) = self.tabs.remove(&id) else {
            return Err(BrowserError::UnknownTab);
        };
        self.openers.remove(&id);
        task.shutdown().await;
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
            self.openers.insert(id, source);
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
        for mut task in self.tabs.drain().map(|(_, task)| task) {
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
        Reply::Tabs(task.tabs.keys().copied().collect())
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
        Reply::Tab(
            task.tabs
                .get(&self.id)
                .map(|task| task.handle.clone())
                .ok_or(BrowserError::UnknownTab),
        )
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

    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::CloseTab(task.close_tab(self.id).await)
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

impl BrowserOperation for OpenerTab {
    type Output = Option<TabId>;

    fn into_command(self) -> Command {
        Command::OpenerTab { tab: self.tab }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::OpenerTab(tab) => Some(tab),
            _ => None,
        }
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "serve is async by BrowserOperation contract; this op resolves synchronously"
    )]
    async fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::OpenerTab(task.openers.get(&self.tab).copied())
    }
}

impl BrowserOperation for WindowMessage {
    type Output = Result<(), BrowserError>;

    fn into_command(self) -> Command {
        Command::WindowMessage {
            target: self.target,
            payload: self.payload,
        }
    }

    fn unwrap(reply: Reply) -> Option<Self::Output> {
        match reply {
            Reply::WindowMessage(result) => Some(result),
            _ => None,
        }
    }

    async fn serve(self, task: &mut BrowserTask) -> Reply {
        let handle = task.tabs.get(&self.target).map(|task| task.handle.clone());
        let result = match handle {
            Some(handle) => handle
                .window_message(self.payload)
                .await
                .map_err(|_| BrowserError::UnknownTab),
            None => Err(BrowserError::UnknownTab),
        };
        Reply::WindowMessage(result)
    }
}
