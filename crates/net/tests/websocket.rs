//! WebSocket handle over loopback: ticket 14's acceptance criteria.

mod common;

use common::TestServer;
use net::{Agent, Method, WsEvent, WsMessage};
use tungstenite::protocol::frame::Frame;
use tungstenite::protocol::frame::coding::{Data, OpCode};
use tungstenite::{Message, accept, accept_hdr};

/// tungstenite's `Callback` `Err` is `ErrorResponse` (~136 bytes); we cannot
/// shrink a third-party handshake callback.
#[allow(clippy::result_large_err, clippy::unnecessary_wraps)] // tungstenite Callback
fn attach_lax_cookie(
    _: &tungstenite::handshake::server::Request,
    mut response: tungstenite::handshake::server::Response,
) -> Result<tungstenite::handshake::server::Response, tungstenite::handshake::server::ErrorResponse>
{
    response.headers_mut().append(
        "Set-Cookie",
        "lax=1; Path=/; SameSite=Lax"
            .parse()
            .expect("set-cookie header"),
    );
    Ok(response)
}

struct CaptureOriginUa {
    slot: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
}

impl tungstenite::handshake::server::Callback for CaptureOriginUa {
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        let origin = request
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let ua = request
            .headers()
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        *self.slot.lock().expect("capture") = Some((origin, ua));
        Ok(response)
    }
}

struct NoteOriginPresent {
    slot: std::sync::Arc<std::sync::Mutex<bool>>,
}

impl tungstenite::handshake::server::Callback for NoteOriginPresent {
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        *self.slot.lock().expect("flag") = request.headers().contains_key("origin");
        Ok(response)
    }
}

#[test]
fn echo_works_in_both_directions() {
    let server = TestServer::start(|conn| {
        let mut ws = accept(conn.stream_mut()).expect("server handshake");
        let Message::Text(first) = ws.read().expect("client text") else {
            panic!("expected text from client");
        };
        ws.send(Message::Text(first)).expect("echo");
        ws.send(Message::Text("from-server".into()))
            .expect("server push");
        let _ = ws.read();
    });

    let ws = Agent::new()
        .request(Method::GET, server.ws_url("/echo"))
        .upgrade()
        .expect("ws handshake");
    ws.send(WsMessage::Text("hello".into()))
        .expect("client send");
    assert_eq!(
        ws.take_next_message().expect("echo"),
        WsEvent::Message(WsMessage::Text("hello".into()))
    );
    assert_eq!(
        ws.take_next_message().expect("push"),
        WsEvent::Message(WsMessage::Text("from-server".into()))
    );
    ws.close(1000, "done").expect("client close");
    server.assert_clean();
}

#[test]
fn take_next_message_answers_server_pings() {
    let ponged = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&ponged);
    let server = TestServer::start(move |conn| {
        let mut ws = accept(conn.stream_mut()).expect("server handshake");
        ws.send(Message::Ping(b"probe".to_vec().into()))
            .expect("ping");
        loop {
            match ws.read() {
                Ok(Message::Pong(payload)) => {
                    assert_eq!(&payload[..], b"probe");
                    flag.store(true, std::sync::atomic::Ordering::Release);
                    let _ = ws.close(None);
                    break;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let ws = Agent::new()
        .request(Method::GET, server.ws_url("/ping"))
        .upgrade()
        .expect("ws handshake");
    // Pings are answered on read, not by a background thread.
    match ws.take_next_message().expect("close after ping") {
        WsEvent::Close { .. } => {}
        WsEvent::Message(msg) => panic!("expected close after ping/pong, got {msg:?}"),
    }
    assert!(
        ponged.load(std::sync::atomic::Ordering::Acquire),
        "server never saw a pong"
    );
    server.assert_clean();
}

#[test]
fn fragmented_text_arrives_as_one_message() {
    let server = TestServer::start(|conn| {
        let mut ws = accept(conn.stream_mut()).expect("server handshake");
        ws.send(Message::Frame(Frame::message(
            b"hel".to_vec(),
            OpCode::Data(Data::Text),
            false,
        )))
        .expect("first fragment");
        ws.send(Message::Frame(Frame::message(
            b"lo".to_vec(),
            OpCode::Data(Data::Continue),
            true,
        )))
        .expect("last fragment");
        let _ = ws.read();
    });

    let ws = Agent::new()
        .request(Method::GET, server.ws_url("/frag"))
        .upgrade()
        .expect("ws handshake");
    assert_eq!(
        ws.take_next_message().expect("reassembled"),
        WsEvent::Message(WsMessage::Text("hello".into()))
    );
    ws.close(1000, "").expect("close");
    server.assert_clean();
}

#[test]
fn close_handshake_surfaces_code_and_reason() {
    let server = TestServer::start(|conn| {
        let mut ws = accept(conn.stream_mut()).expect("server handshake");
        ws.close(Some(tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: "bye".into(),
        }))
        .expect("server close");
        let _ = ws.read();
    });

    let ws = Agent::new()
        .request(Method::GET, server.ws_url("/close"))
        .upgrade()
        .expect("ws handshake");
    match ws.take_next_message().expect("close event") {
        WsEvent::Close { code, reason } => {
            assert_eq!(code, 1000);
            assert_eq!(reason, "bye");
        }
        WsEvent::Message(msg) => panic!("expected close, got {msg:?}"),
    }
    server.assert_clean();
}

#[test]
fn handshake_set_cookie_is_stored_and_same_site_ws_sends_lax() {
    let hops = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let server_hops = std::sync::Arc::clone(&hops);
    let server = TestServer::start(move |conn| {
        let n = {
            let mut g = server_hops.lock().expect("hops");
            let n = *g;
            *g += 1;
            n
        };
        if n == 0 {
            let mut ws =
                accept_hdr(conn.stream_mut(), attach_lax_cookie).expect("server handshake");
            let _ = ws.read();
        } else {
            conn.read_request();
            let mut out = Vec::new();
            out.extend_from_slice(
                b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
            );
            conn.write_all(&out).expect("http");
        }
    });

    let agent = Agent::new();
    let ws_url = server.ws_url("/ws");
    let ws = agent.request(Method::GET, ws_url).upgrade().expect("ws");
    ws.close(1000, "").expect("close");
    agent
        .request(Method::GET, server.url("/next"))
        .send()
        .expect("http after ws");
    let recorded = server.requests();
    assert!(
        recorded.iter().any(|r| r.header("cookie") == Some("lax=1")),
        "same-site follow-up must carry the 101 Set-Cookie, got {recorded:?}"
    );
    server.assert_clean();
}

#[test]
fn upgrade_with_initiator_sends_origin_and_agent_user_agent() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<(String, String)>));
    let capture = std::sync::Arc::clone(&captured);
    let server = TestServer::start(move |conn| {
        let mut ws = accept_hdr(
            conn.stream_mut(),
            CaptureOriginUa {
                slot: std::sync::Arc::clone(&capture),
            },
        )
        .expect("server handshake");
        let _ = ws.read();
    });

    let agent = net::AgentBuilder::new()
        .user_agent("tinybrowser-test/1")
        .build();
    let document = url::Url::parse("https://app.example/page").expect("document");
    let ws = agent
        .request(Method::GET, server.ws_url("/ws"))
        .with_initiator(document)
        .upgrade()
        .expect("ws");
    ws.close(1000, "").expect("close");

    let (origin, ua) = captured
        .lock()
        .expect("capture")
        .clone()
        .expect("handshake saw headers");
    assert_eq!(origin, "https://app.example");
    assert_eq!(ua, "tinybrowser-test/1");
    server.assert_clean();
}

#[test]
fn bare_upgrade_omits_origin() {
    let saw_origin = std::sync::Arc::new(std::sync::Mutex::new(false));
    let flag = std::sync::Arc::clone(&saw_origin);
    let server = TestServer::start(move |conn| {
        let mut ws = accept_hdr(
            conn.stream_mut(),
            NoteOriginPresent {
                slot: std::sync::Arc::clone(&flag),
            },
        )
        .expect("server handshake");
        let _ = ws.read();
    });

    let ws = Agent::new()
        .request(Method::GET, server.ws_url("/ws"))
        .upgrade()
        .expect("ws");
    ws.close(1000, "").expect("close");
    assert!(
        !*saw_origin.lock().expect("flag"),
        "embedder dial without a document must not invent Origin"
    );
    server.assert_clean();
}
