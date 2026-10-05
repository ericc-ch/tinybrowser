//! Blitz service providers backed by our stacks.
//!
//! The renderer owns no async runtime (carriers do), so the net provider
//! takes a spawn closure instead of a tokio handle. Screenshot flow polls
//! [`TinyNetProvider::in_flight`] to know when subresources have landed.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A sendable task the carrier runs on its runtime.
type Task = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Spawns [`Task`]s on the carrier runtime.
pub(crate) type SpawnFn = std::sync::Arc<dyn Fn(Task) + Send + Sync>;

/// Per-response fetch ceiling in bytes.
const FETCH_LIMIT: usize = 8 * 1024 * 1024;

/// Fetches Blitz subresources (stylesheets, images, fonts) through our agent.
pub(crate) struct TinyNetProvider {
    agent: net::Agent,
    spawn: SpawnFn,
    in_flight: std::sync::Arc<AtomicUsize>,
}

impl TinyNetProvider {
    /// Builds a provider over `agent`, spawning fetches through `spawn`.
    pub(crate) fn new(agent: net::Agent, spawn: SpawnFn) -> Self {
        Self {
            agent,
            spawn,
            in_flight: std::sync::Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Count of fetches started but not yet finished.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }
}

impl blitz_traits::net::NetProvider for TinyNetProvider {
    fn fetch(
        &self,
        _doc_id: usize,
        request: blitz_traits::net::Request,
        handler: Box<dyn blitz_traits::net::NetHandler>,
    ) {
        let Ok(method) = net::Method::parse(request.method.as_str()) else {
            return;
        };
        let url = request.url;
        let resolved = url.to_string();
        let agent = self.agent.clone();
        let in_flight = self.in_flight.clone();
        in_flight.fetch_add(1, Ordering::SeqCst);
        (self.spawn)(Box::pin(async move {
            let _decrement = DecrementOnDrop(in_flight);
            let outgoing = net::Request::new(method, url);
            let Ok(response) = agent.send(outgoing).await else {
                return;
            };
            if !(200..300).contains(&response.status()) {
                return;
            }
            let Ok(bytes) = response.into_body().bytes(FETCH_LIMIT).await else {
                return;
            };
            handler.bytes(resolved, bytes.into());
        }));
    }
}

/// Decrements its counter when dropped, so every fetch exit path is counted.
struct DecrementOnDrop(std::sync::Arc<AtomicUsize>);

impl Drop for DecrementOnDrop {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// No-op shell: one-shot screenshots re-resolve explicitly, never on redraw.
pub(crate) struct TinyShell;

impl blitz_traits::shell::ShellProvider for TinyShell {}

/// Records navigations for the carrier to drain; screenshots never navigate.
#[derive(Default)]
pub(crate) struct TinyNav {
    pending: Mutex<Vec<url::Url>>,
}

impl TinyNav {
    /// Builds an empty recorder.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Takes every recorded navigation, leaving the recorder empty.
    pub(crate) fn drain(&self) -> Vec<url::Url> {
        std::mem::take(
            &mut self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl blitz_traits::navigation::NavigationProvider for TinyNav {
    fn navigate_to(&self, options: blitz_traits::navigation::NavigationOptions) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(options.url);
    }
}

#[cfg(test)]
mod tests {
    use super::{SpawnFn, TinyNav, TinyNetProvider, TinyShell};
    use blitz_traits::navigation::{NavigationOptions, NavigationProvider};
    use blitz_traits::net::{NetHandler, NetProvider, Request};
    use blitz_traits::shell::ShellProvider;
    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::time::Duration;

    struct ChanHandler {
        tx: tokio::sync::oneshot::Sender<(String, Vec<u8>)>,
    }

    impl NetHandler for ChanHandler {
        fn bytes(self: Box<Self>, resolved_url: String, bytes: blitz_traits::net::Bytes) {
            let _ = self.tx.send((resolved_url, bytes.to_vec()));
        }
    }

    fn serve_once(body: &'static str) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        port
    }

    #[test]
    fn fetches_stylesheet_through_agent() {
        let port = serve_once("div{color:red}");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let agent = net::Agent::new(net::AgentOptions::default()).expect("agent");
        let handle = runtime.handle().clone();
        let spawn: SpawnFn = Arc::new(move |task| {
            handle.spawn(task);
        });
        let provider = TinyNetProvider::new(agent, spawn);
        let url = format!("http://127.0.0.1:{port}/a.css");
        let (tx, rx) = tokio::sync::oneshot::channel();
        provider.fetch(
            1,
            Request::get(url.parse().expect("url")),
            Box::new(ChanHandler { tx }),
        );
        let (resolved, bytes) = runtime
            .block_on(async move {
                tokio::time::timeout(Duration::from_secs(10), rx)
                    .await
                    .expect("fetch completes")
                    .expect("handler called")
            });
        assert_eq!(resolved, url);
        assert_eq!(bytes, b"div{color:red}");
        assert_eq!(provider.in_flight(), 0);
    }

    #[test]
    fn shell_and_nav_are_wired() {
        let shell = TinyShell;
        shell.request_redraw();
        let nav = TinyNav::new();
        nav.navigate_to(
            NavigationOptions::new(
                "https://example.com/".parse().expect("url"),
                None,
                1,
            )
            .set_method(blitz_traits::net::http::Method::GET),
        );
        let pending = nav.drain();
        assert_eq!(pending.len(), 1);
        assert!(nav.drain().is_empty());
    }
}
