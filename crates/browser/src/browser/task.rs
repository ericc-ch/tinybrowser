use std::collections::HashMap;
use std::sync::Arc;

use crate::actor::{TabHandle, TabId, TabTask};
use crate::context::BrowserContext;
use crate::exchange::ServerInput;
use crate::manager::RendererProcessManager;

use super::{BrowserError, BrowserHandle, BrowserServer, Command, Notice, Reply};

pub(super) struct BrowserTask {
    server: BrowserServer,
    context: BrowserContext,
    renderers: Arc<RendererProcessManager>,
    tabs: HashMap<TabId, TabTask>,
    next_tab: Option<u64>,
    openers: HashMap<TabId, TabId>,
}

enum DispatchOutcome {
    Continue(Reply),
    Stop(Reply),
}

impl BrowserTask {
    pub(super) fn new(
        server: BrowserServer,
        context: BrowserContext,
        browser: BrowserHandle,
    ) -> Self {
        let renderers = Arc::new(RendererProcessManager::new(
            context.partition_services(),
            context.session_storage(),
            browser,
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
            Command::CreateTab => Reply::CreateTab(self.create_tab()),
            Command::Tabs => Reply::Tabs(self.tabs.keys().copied().collect()),
            Command::Tab { id } => Reply::Tab(
                self.tabs
                    .get(&id)
                    .map(|task| task.handle.clone())
                    .ok_or(BrowserError::UnknownTab),
            ),
            Command::CloseTab { id } => Reply::CloseTab(self.close_tab(id).await),
            Command::CookieRecords { url } => {
                Reply::CookieRecords(self.context.cookie_records(&url))
            }
            Command::ClearCookies => {
                self.context.clear_cookies();
                Reply::ClearCookies
            }
            Command::AddCookie { cookie, url } => {
                Reply::AddCookie(self.context.add_cookie(&cookie, &url))
            }
            Command::OpenWindow {
                url,
                source,
                noopener,
            } => Reply::OpenWindow(self.open_window(url, source, noopener)),
            Command::OpenerTab { tab } => Reply::OpenerTab(self.openers.get(&tab).copied()),
            Command::WindowMessage { target, payload } => {
                let handle = self.tabs.get(&target).map(|task| task.handle.clone());
                let result = match handle {
                    Some(handle) => handle
                        .window_message(payload)
                        .await
                        .map_err(|_| BrowserError::UnknownTab),
                    None => Err(BrowserError::UnknownTab),
                };
                Reply::WindowMessage(result)
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
        let task = TabTask::spawn(id, self.context.tab_network(), Arc::clone(&self.renderers));
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

    fn open_window(
        &mut self,
        url: String,
        source: Option<TabId>,
        noopener: bool,
    ) -> Result<TabId, BrowserError> {
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
