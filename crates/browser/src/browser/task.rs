use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;

use crate::actor::{NewTab, RemoteWindowMessage, TabHandle, TabId, TabTask};
use crate::context::BrowserContext;
use crate::exchange::ServerInput;
use crate::manager::RendererProcessManager;

use super::{
    AddCookie, BrowserError, BrowserHandle, BrowserOperation, BrowserServer, ClearCookies,
    CloseTab, Command, CookieRecords, CreateTab, GetTab, ListTabs, Notice, OpenWindowOptions,
    Reply,
};

/// One tab's target names.
///
/// Two values because a `window.name` assignment reaches the browser through a
/// synchronous host call, while the session-history entry it belongs to is
/// written later, when the tab actor commits or traverses. The live value is
/// what a `window.open` searches and what the next browser-driven commit
/// resolves; the document value is what the entry the document is leaving
/// takes, and it survives the renderer's cross-origin reset so a reset that
/// lands before the browser's commit cannot lose it
/// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabNames {
    /// The browsing context's target name. A cross-origin reset clears it
    /// before the new document installs.
    pub(crate) live: Option<renderer::ContextName>,
    /// The active document state's navigable target name. `window.name` writes
    /// it, and the next commit copies it onto the entry the document leaves
    /// with, so a reset that lands first cannot lose it.
    pub(crate) document: Option<renderer::ContextName>,
}

impl TabNames {
    /// The pair a commit installs: one name for the browsing context and the
    /// document state it just settled.
    pub(crate) fn from_name(name: Option<&renderer::ContextName>) -> Self {
        Self {
            live: name.cloned(),
            document: name.cloned(),
        }
    }
}

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
    /// The frame inside the opener that created each tab; see
    /// [`Self::opener_frame`].
    opener_frames: Arc<Mutex<HashMap<TabId, renderer::FrameId>>>,
    /// Target names of tabs. The browsing context group is what
    /// `window.open`'s name lookup searches, so the names are keyed by tab and
    /// resolved through the opener chain; a tab with no name has no entry
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#the-rules-for-choosing-a-navigable>).
    names: Arc<Mutex<HashMap<TabId, TabNames>>>,
    /// The popup sandboxing flag set each tab's documents inherit. The browser
    /// owns the copy a `window.open` is checked against and the copy a new
    /// tab inherits, so the renderer's claim can only ever add flags
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    sandboxes: Arc<Mutex<HashMap<TabId, renderer::SandboxFlags>>>,
}

impl TabRegistry {
    fn new() -> Self {
        Self {
            tabs: Arc::new(Mutex::new(HashMap::new())),
            openers: Arc::new(Mutex::new(HashMap::new())),
            opener_frames: Arc::new(Mutex::new(HashMap::new())),
            names: Arc::new(Mutex::new(HashMap::new())),
            sandboxes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The tab `caller`'s browsing context group holds under `name`, when one
    /// is open. The group is the opener chain's root, so a name opened by one
    /// tab is visible to every tab in the same group and to no other.
    pub(crate) fn find_by_name(
        &self,
        caller: TabId,
        name: &renderer::ContextName,
    ) -> Option<TabId> {
        let openers = self.openers.lock().unwrap_or_else(PoisonError::into_inner);
        let root = group_root(&openers, caller);
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(id, named)| {
                named
                    .live
                    .as_ref()
                    .is_some_and(|live| live.as_str() == name.as_str())
                    && group_root(&openers, **id) == root
            })
            .map(|(id, _)| *id)
    }

    /// Records the target name of a tab that is not in the table yet.
    ///
    /// This is creation, not a later change: a live tab's names move through
    /// [`Self::install_commit`], [`Self::settle_document_name`], and
    /// [`Self::rename`], which all skip a tab that closed. A tab id is never
    /// reused, so a name written here can only belong to the tab being built.
    pub(crate) fn insert_name(&self, id: TabId, name: Option<renderer::ContextName>) {
        let mut names = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(name) = name {
            names.insert(id, TabNames::from_name(Some(&name)));
        } else {
            names.remove(&id);
        }
    }

    /// Applies a name change the renderer reported, unless its tab has closed.
    ///
    /// The tab table and the names share one critical section: a close that
    /// landed between a separate liveness check and this write would leave a
    /// name pointing at a dead tab, and the next `window.open` would answer a
    /// `WindowProxy` for it instead of opening a new context.
    pub(crate) fn rename(&self, id: TabId, change: renderer::NameChange) {
        let tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        if !tabs.contains_key(&id) {
            return;
        }
        let mut names = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        match change {
            renderer::NameChange::Assigned(name) => {
                set_names(&mut names, id, TabNames::from_name(name.as_ref()));
            }
            // The renderer's cross-origin reset lands before the browser
            // commits the new document. The name belongs to the document being
            // left, so its entry still takes it when the browser commits
            // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
            renderer::NameChange::Reset => {
                if let Some(entry) = names.get_mut(&id) {
                    entry.live = None;
                }
            }
        }
    }

    /// Installs the document-state name a renderer-driven commit settled.
    ///
    /// The commit event travels behind the renderer's own `Assigned` calls, so
    /// a name set by the new document may already be in the registry. The
    /// write is a compare-and-set on the outgoing name: only when nothing has
    /// renamed the tab since the commit does the new value apply, and a later
    /// assignment always wins.
    pub(crate) fn settle_document_name(
        &self,
        id: TabId,
        outgoing: Option<&renderer::ContextName>,
        name: Option<renderer::ContextName>,
    ) {
        let tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        if !tabs.contains_key(&id) {
            return;
        }
        let mut names = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        if names.get(&id).and_then(|entry| entry.document.as_ref()) != outgoing {
            return;
        }
        if let Some(name) = name {
            names.entry(id).or_default().document = Some(name);
        } else {
            if let Some(entry) = names.get_mut(&id) {
                entry.document = None;
            }
            if names.get(&id).is_some_and(|entry| entry.live.is_none()) {
                names.remove(&id);
            }
        }
    }

    /// Installs the names a browser-driven commit settled, returning the pair
    /// it replaced.
    ///
    /// A read followed by a write would race a script that renames the tab
    /// between them, so both happen under one lock. `None` means the tab
    /// closed while the navigation was in flight.
    pub(crate) fn install_commit(
        &self,
        id: TabId,
        name: Option<&renderer::ContextName>,
    ) -> Option<TabNames> {
        let tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        if !tabs.contains_key(&id) {
            return None;
        }
        let mut map = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = map.get(&id).cloned().unwrap_or_default();
        set_names(&mut map, id, TabNames::from_name(name));
        Some(previous)
    }

    /// Puts back the pair [`Self::install_commit`] replaced, unless a script
    /// renamed the tab after the commit, in which case the newer value wins.
    pub(crate) fn restore_commit(
        &self,
        id: TabId,
        previous: Option<TabNames>,
        installed: &TabNames,
    ) {
        let Some(previous) = previous else {
            return;
        };
        let tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        if !tabs.contains_key(&id) {
            return;
        }
        let mut map = self.names.lock().unwrap_or_else(PoisonError::into_inner);
        let current = map.get(&id).cloned().unwrap_or_default();
        if &current == installed {
            set_names(&mut map, id, previous);
        }
    }

    /// The target name of one tab, when it has one.
    pub(crate) fn name(&self, id: TabId) -> Option<renderer::ContextName> {
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .and_then(|entry| entry.live.clone())
    }

    /// The active document state's navigable target name: the value a
    /// `window.name` assignment writes and the entry the document leaves takes.
    ///
    /// It outlives the browsing context's name so a reset that arrives before
    /// the commit cannot erase what the outgoing document stored
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    pub(crate) fn document_name(&self, id: TabId) -> Option<renderer::ContextName> {
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .and_then(|entry| entry.document.clone())
    }

    /// Registers a tab id with no coordinator behind it, for unit tests that
    /// exercise the registry's own rules.
    #[cfg(test)]
    fn add_fixture(&self, id: TabId) {
        self.insert(id, TabTask::fixture(id));
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

    /// Records the popup sandboxing flag set a tab's documents inherit.
    ///
    /// The browser keeps its own copy because the set is what it checks a
    /// `window.open` against: a renderer that claims its document may open a
    /// popup cannot widen what the browser already knows, and a tab the
    /// browser created itself starts empty
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    pub(crate) fn set_sandbox(&self, id: TabId, sandbox: renderer::SandboxFlags) {
        self.sandboxes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sandbox);
    }

    /// The popup sandboxing flag set of one live tab; empty when it has none.
    pub(crate) fn sandbox(&self, id: TabId) -> renderer::SandboxFlags {
        self.sandboxes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .copied()
            .unwrap_or_default()
    }

    fn remove_sandbox(&self, id: TabId) {
        self.sandboxes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }

    /// Queues one `postMessage` delivery on the target tab's mailbox. Returns
    /// false when the tab is gone, its coordinator stopped, or the mailbox is
    /// full. A full mailbox drops the message instead of blocking: the caller
    /// is the sender's renderer service task, and waiting there would stall
    /// every service call for that renderer until the target drains.
    pub(crate) fn deliver(&self, id: TabId, message: RemoteWindowMessage) -> bool {
        let sender = self
            .tabs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map(TabTask::deliveries);
        match sender {
            Some(sender) => match sender.try_send(message) {
                Ok(()) => {
                    logging::debug!(
                        target: "browser::tabs",
                        "window message queued for tab {id}"
                    );
                    true
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    logging::debug!(
                        target: "browser::tabs",
                        "window message dropped: mailbox full for tab {id}"
                    );
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            },
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

    /// Takes a tab out of the table, with its names, in one critical section.
    ///
    /// A name that outlived its tab would make the next `window.open` answer a
    /// `WindowProxy` for a closed context instead of opening a new one.
    fn remove(&self, id: TabId) -> Option<TabTask> {
        let mut tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        let task = tabs.remove(&id)?;
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        Some(task)
    }

    fn drain(&self) -> Vec<TabTask> {
        let mut tabs = self.tabs.lock().unwrap_or_else(PoisonError::into_inner);
        let tasks: Vec<TabTask> = tabs.drain().map(|(_, task)| task).collect();
        drop(tabs);
        self.names
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        tasks
    }

    /// Records that `id` was opened by `opener`'s frame `frame`.
    fn set_opener(&self, id: TabId, opener: TabId, frame: renderer::FrameId) {
        self.openers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, opener);
        self.opener_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, frame);
    }

    /// The frame inside `id`'s opener that opened it. Together with
    /// [`Self::opener`] this is "the auxiliary contexts it created" for the
    /// sandboxed navigation check.
    pub(crate) fn opener_frame(&self, id: TabId) -> Option<renderer::FrameId> {
        self.opener_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .copied()
    }

    fn remove_opener(&self, id: TabId) {
        self.openers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        self.opener_frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

/// The top-level tab of the browsing context group `tab` belongs to: the end
/// of its opener chain. Tab ids are never reused, so a group whose root has
/// closed keeps its identity until the last member closes.
fn group_root(openers: &HashMap<TabId, TabId>, tab: TabId) -> TabId {
    let mut root = tab;
    // A chain longer than the table is a cycle, which cannot be built; stop
    // rather than loop if one ever is.
    for _ in 0..=openers.len() {
        match openers.get(&root) {
            Some(opener) => root = *opener,
            None => break,
        }
    }
    root
}

pub(crate) struct BrowserTask {
    server: BrowserServer,
    context: BrowserContext,
    renderers: Arc<RendererProcessManager>,
    tabs: TabRegistry,
    next_tab: Option<u64>,
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
        let tabs = TabRegistry::new();
        let identity = browser.identity().clone();
        let renderers = Arc::new(RendererProcessManager::new(
            context.partition_services(),
            context.session_storage(),
            browser,
            tabs.clone(),
            identity,
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
                ServerInput::RequestChunk
                | ServerInput::RequestEnd
                | ServerInput::Cancel { .. } => {}
            }
        }
        let _result = self.shutdown().await;
    }

    async fn dispatch(&mut self, command: Command) -> DispatchOutcome {
        let reply = match command {
            Command::CreateTab => CreateTab.serve(self),
            Command::Tabs => ListTabs.serve(self),
            Command::Tab { id } => GetTab { id }.serve(self),
            Command::CloseTab { id } => CloseTab { id }.serve(self),
            Command::CookieRecords { url } => CookieRecords { url }.serve(self),
            Command::ClearCookies => ClearCookies.serve(self),
            Command::AddCookie { cookie, url } => AddCookie { cookie, url }.serve(self),
            Command::OpenWindow(request) => request.serve(self),
            Command::Close => {
                let result = self.shutdown().await;
                return DispatchOutcome::Stop(Reply::Close(result));
            }
        };
        DispatchOutcome::Continue(reply)
    }

    fn create_tab(&mut self, tab: NewTab) -> Result<TabHandle, BrowserError> {
        let raw = self.next_tab.ok_or(BrowserError::TabIdExhausted)?;
        self.next_tab = raw.checked_add(1);
        let id = TabId::new(raw);
        self.context.create_tab(id);
        self.tabs.insert_name(id, tab.name.clone());
        self.tabs.set_sandbox(id, tab.popup_sandbox);
        if let (Some(opener), Some(frame)) = (tab.opener, tab.opener_frame) {
            self.tabs.set_opener(id, opener, frame);
        }
        let task = TabTask::spawn(
            id,
            self.context.tab_network(),
            Arc::clone(&self.renderers),
            self.tabs.clone(),
            tab,
        );
        let handle = task.handle.clone();
        self.tabs.insert(id, task);
        Ok(handle)
    }

    fn close_tab(&mut self, id: TabId) -> Result<(), BrowserError> {
        let Some(mut task) = self.tabs.remove(id) else {
            return Err(BrowserError::UnknownTab);
        };
        self.tabs.remove_opener(id);
        self.tabs.remove_sandbox(id);
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
            target,
            source,
            source_frame,
            opener,
            initiator,
            popup_sandbox,
        } = request;
        // A popup keeps its target name across a cross-origin navigation only
        // while its opener browsing context is live
        // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
        let opener_tab = source.filter(|_| opener == renderer::OpenerPolicy::Keep);
        let handle = self.create_tab(NewTab {
            name: target.new_context_name().cloned(),
            opener: opener_tab,
            opener_frame: opener_tab.map(|_| source_frame),
            popup_sandbox,
            // The initial `about:blank` inherits the opener's origin, which is
            // what the first navigation's target-name reset compares against
            // (<https://html.spec.whatwg.org/multipage/browsers.html#determining-the-origin>).
            document_origin: initiator.origin.clone(),
        })?;
        let id = handle.id();
        if let Some(opener_tab) = opener_tab {
            self.context.copy_session(opener_tab, id);
        }
        // A fresh tab is already on `about:blank`, so neither a missing URL
        // nor an explicit `about:blank` needs a navigation.
        if let Some(url) = url.filter(|url| url.as_str() != "about:blank") {
            tokio::spawn(async move {
                let _result = handle.goto_from(url.as_str(), initiator).await;
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::CreateTab(task.create_tab(NewTab::browser_created()))
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
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

    fn serve(self, task: &mut BrowserTask) -> Reply {
        Reply::OpenWindow(task.open_window(self))
    }
}

/// Writes a name pair, dropping the entry when the tab has no names at all.
fn set_names(map: &mut HashMap<TabId, TabNames>, id: TabId, names: TabNames) {
    if names.live.is_none() && names.document.is_none() {
        map.remove(&id);
    } else {
        map.insert(id, names);
    }
}

#[cfg(test)]
mod tests {
    use super::TabRegistry;
    use crate::actor::TabId;

    fn name(raw: &str) -> renderer::ContextName {
        renderer::ContextName::parse(raw).expect("a test name is non-empty")
    }

    /// A cleared name is removed, not stored as an empty one. `ContextName`
    /// cannot hold `""`, so the page-side lookup can no longer ask for the
    /// name that would match every cleared tab (#235).
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    #[test]
    fn a_cleared_name_is_no_name() {
        let tabs = TabRegistry::new();
        let opener = TabId::new(1);
        let popup = TabId::new(2);
        tabs.set_opener(popup, opener, renderer::FrameId::MAIN);
        tabs.insert_name(popup, Some(name("peer")));
        assert_eq!(tabs.name(popup), Some(name("peer")));
        assert_eq!(tabs.find_by_name(opener, &name("peer")), Some(popup));
        tabs.insert_name(popup, None);
        assert_eq!(tabs.name(popup), None);
        assert_eq!(tabs.find_by_name(opener, &name("peer")), None);
    }

    /// A reset clears the live name and leaves the document state's name for
    /// the entry the leaving document owns; an assignment of the empty string
    /// clears both (#235).
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    #[tokio::test]
    async fn a_reset_clears_only_the_live_name() {
        let tabs = TabRegistry::new();
        let tab = TabId::new(1);
        tabs.add_fixture(tab);
        tabs.rename(tab, renderer::NameChange::Assigned(Some(name("peer"))));
        assert_eq!(tabs.name(tab), Some(name("peer")));
        assert_eq!(tabs.document_name(tab), Some(name("peer")));

        tabs.rename(tab, renderer::NameChange::Reset);
        assert_eq!(tabs.name(tab), None);
        assert_eq!(
            tabs.document_name(tab),
            Some(name("peer")),
            "the document being left keeps its own name"
        );

        tabs.rename(tab, renderer::NameChange::Assigned(None));
        assert_eq!(tabs.name(tab), None);
        assert_eq!(tabs.document_name(tab), None);
    }

    /// A renderer-driven commit settles the new document's name unless a script
    /// renamed the tab after the commit event was queued.
    #[tokio::test]
    async fn a_commit_settles_the_name_unless_a_script_renamed_it() {
        let tabs = TabRegistry::new();
        let tab = TabId::new(1);
        tabs.add_fixture(tab);
        tabs.rename(tab, renderer::NameChange::Assigned(Some(name("old"))));

        // The commit replaced `old` with no name, and nothing renamed since.
        let previous = tabs.install_commit(tab, None).expect("live tab");
        assert_eq!(previous.document, Some(name("old")));
        tabs.settle_document_name(tab, Some(&name("old")), None);
        assert_eq!(tabs.document_name(tab), None);

        // The new document set its own name before the browser handled the
        // event, so the commit's value loses.
        tabs.rename(tab, renderer::NameChange::Assigned(Some(name("new"))));
        tabs.settle_document_name(tab, Some(&name("old")), None);
        assert_eq!(tabs.document_name(tab), Some(name("new")));
    }

    /// A failed mount puts back what it replaced, unless a script renamed the
    /// tab while the mount was running.
    #[tokio::test]
    async fn a_failed_mount_restores_the_names_it_replaced() {
        let tabs = TabRegistry::new();
        let tab = TabId::new(1);
        tabs.add_fixture(tab);
        let installed = super::TabNames::from_name(Some(&name("kept")));
        let previous = tabs.install_commit(tab, Some(&name("kept")));
        assert_eq!(previous, Some(super::TabNames::default()));
        tabs.restore_commit(tab, previous, &installed);
        assert_eq!(tabs.name(tab), None);

        // The installed value is no longer current, so restoring would lose
        // the newer assignment.
        let previous = tabs.install_commit(tab, Some(&name("kept")));
        tabs.rename(tab, renderer::NameChange::Assigned(Some(name("newer"))));
        tabs.restore_commit(tab, previous, &installed);
        assert_eq!(tabs.name(tab), Some(name("newer")));
    }

    /// A closed tab takes its names with it, and a coordinator still finishing
    /// a commit cannot put one back
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    #[tokio::test]
    async fn a_closed_tab_takes_its_names_with_it() {
        let tabs = TabRegistry::new();
        let opener = TabId::new(1);
        let popup = TabId::new(2);
        tabs.add_fixture(opener);
        tabs.add_fixture(popup);
        tabs.set_opener(popup, opener, renderer::FrameId::MAIN);
        tabs.insert_name(popup, Some(name("peer")));
        assert_eq!(tabs.find_by_name(opener, &name("peer")), Some(popup));

        let _task = tabs.remove(popup).expect("live tab");
        assert_eq!(tabs.name(popup), None);
        tabs.rename(popup, renderer::NameChange::Assigned(Some(name("peer"))));
        assert_eq!(tabs.name(popup), None);
        tabs.settle_document_name(popup, None, Some(name("peer")));
        assert_eq!(tabs.name(popup), None);
        assert_eq!(tabs.install_commit(popup, Some(&name("peer"))), None);
        assert_eq!(tabs.find_by_name(opener, &name("peer")), None);
    }

    /// The lookup is scoped to the opener chain: a name one group holds is not
    /// visible to a tab outside it
    /// (<https://html.spec.whatwg.org/multipage/document-sequences.html#the-rules-for-choosing-a-navigable>).
    #[test]
    fn a_name_is_found_through_the_opener_chain_only() {
        let tabs = TabRegistry::new();
        let opener = TabId::new(1);
        let stranger = TabId::new(2);
        let popup = TabId::new(3);
        tabs.set_opener(popup, opener, renderer::FrameId::MAIN);
        tabs.insert_name(popup, Some(name("peer")));
        assert_eq!(tabs.find_by_name(opener, &name("peer")), Some(popup));
        assert_eq!(tabs.find_by_name(popup, &name("peer")), Some(popup));
        assert_eq!(tabs.find_by_name(stranger, &name("peer")), None);
    }

    /// The browser's own sandbox copy is what a `window.open` is checked
    /// against and what a new tab inherits, so it must survive a renderer
    /// replacement and disappear with its tab
    /// (<https://html.spec.whatwg.org/multipage/browsers.html#popup-sandboxing-flag-set>).
    #[test]
    fn a_tab_keeps_its_popup_sandbox_until_it_closes() {
        let tabs = TabRegistry::new();
        let plain = TabId::new(1);
        let sandboxed = TabId::new(2);
        assert!(!tabs.sandbox(sandboxed).blocks_navigation());
        tabs.set_sandbox(
            sandboxed,
            renderer::SandboxFlags::parse(Some("allow-scripts allow-popups")),
        );
        assert!(tabs.sandbox(sandboxed).blocks_navigation());
        assert!(tabs.sandbox(sandboxed).origin_is_opaque());
        assert!(!tabs.sandbox(plain).blocks_navigation());
        tabs.remove_sandbox(sandboxed);
        assert!(!tabs.sandbox(sandboxed).blocks_navigation());
    }
}
