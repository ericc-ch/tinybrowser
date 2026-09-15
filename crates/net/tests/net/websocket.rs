use super::common::TestServer;
use std::sync::{Arc, Mutex};

use net::{AgentBuilder, Method, WsEvent, WsMessage};
use tungstenite::protocol::frame::Frame;
use tungstenite::protocol::frame::coding::{CloseCode, Data, OpCode};
use tungstenite::{Message, accept_hdr};

struct HandshakeCapture {
    slot: Arc<Mutex<Option<(String, String, String)>>>,
}

impl tungstenite::handshake::server::Callback for HandshakeCapture {
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        mut response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        let origin = request
            .headers()
            .get("origin")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let user_agent = request
            .headers()
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let protocol = request
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        *self.slot.lock().expect("capture") = Some((origin, user_agent, protocol));
        response.headers_mut().append(
            "Sec-WebSocket-Protocol",
            "tinybrowser-test".parse().expect("protocol header"),
        );
        response.headers_mut().append(
            "Set-Cookie",
            "lax=1; Path=/; SameSite=Lax"
                .parse()
                .expect("set-cookie header"),
        );
        Ok(response)
    }
}

fn start_websocket_server(
    captured: Arc<Mutex<Option<(String, String, String)>>>,
    requests: Arc<Mutex<u8>>,
) -> TestServer {
    TestServer::start(move |connection| {
        let mut number = requests.lock().expect("request number");
        if *number == 0 {
            let mut socket = accept_hdr(
                connection.stream_mut(),
                HandshakeCapture {
                    slot: Arc::clone(&captured),
                },
            )
            .expect("websocket handshake");
            socket
                .send(Message::Frame(Frame::message(
                    b"hel".to_vec(),
                    OpCode::Data(Data::Text),
                    false,
                )))
                .expect("first fragment");
            socket
                .send(Message::Frame(Frame::message(
                    b"lo".to_vec(),
                    OpCode::Data(Data::Continue),
                    true,
                )))
                .expect("last fragment");
            socket
                .send(Message::Ping(b"probe".to_vec().into()))
                .expect("ping");

            let mut saw_pong = false;
            let mut saw_text = false;
            while !(saw_pong && saw_text) {
                match socket.read() {
                    Ok(Message::Pong(payload)) => {
                        assert_eq!(&payload[..], b"probe");
                        saw_pong = true;
                    }
                    Ok(Message::Text(text)) => {
                        assert_eq!(text, "from-client");
                        saw_text = true;
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            assert!(
                saw_pong && saw_text,
                "client did not complete the frame exchange"
            );
            socket
                .close(Some(tungstenite::protocol::CloseFrame {
                    code: CloseCode::Normal,
                    reason: "bye".into(),
                }))
                .expect("server close");
        } else {
            let request = connection.read_request();
            assert_eq!(request.header("cookie"), Some("lax=1"));
            connection
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .expect("http response");
        }
        *number += 1;
    })
}

#[tokio::test]
async fn websocket_transcript_covers_handshake_frames_control_and_cookie_reuse() {
    let captured = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(0_u8));
    let server = start_websocket_server(Arc::clone(&captured), Arc::clone(&requests));

    let agent = AgentBuilder::new().user_agent("tinybrowser-test/1").build();
    let document =
        url::Url::parse(&format!("ws://{}/page", server.local_addr())).expect("document");
    let mut socket = agent
        .request(Method::GET, server.ws_url("/socket"))
        .header("Sec-WebSocket-Protocol", "tinybrowser-test")
        .expect("protocol")
        .with_initiator(document)
        .upgrade()
        .await
        .expect("upgrade");
    socket
        .send(WsMessage::Text("from-client".into()))
        .await
        .expect("client text");
    assert_eq!(
        socket.take_next_message().await.expect("fragmented text"),
        WsEvent::Message(WsMessage::Text("hello".into()))
    );
    assert_eq!(
        socket.take_next_message().await.expect("close after ping"),
        WsEvent::Close {
            code: 1000,
            reason: "bye".into(),
        }
    );

    let (origin, user_agent, protocol) = captured
        .lock()
        .expect("capture")
        .clone()
        .expect("captured handshake");
    assert_eq!(origin, format!("ws://{}", server.local_addr()));
    assert_eq!(user_agent, "tinybrowser-test/1");
    assert_eq!(protocol, "tinybrowser-test");

    agent
        .request(Method::GET, server.url("/after"))
        .send()
        .await
        .expect("cookie follow-up");
    assert_eq!(*requests.lock().expect("request count"), 2);
    server.assert_clean();
}

#[tokio::test]
async fn websocket_dials_use_the_agent_resolve_map() {
    let captured = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(0_u8));
    let server = start_websocket_server(Arc::clone(&captured), Arc::clone(&requests));
    let port = server.local_addr().port();

    let agent = AgentBuilder::new()
        .resolve("ws.test=127.0.0.1")
        .expect("resolve spec")
        .build();
    let url = url::Url::parse(&format!("ws://ws.test:{port}/socket")).expect("absolute url");
    let mut socket = agent
        .request(Method::GET, url)
        .header("Sec-WebSocket-Protocol", "tinybrowser-test")
        .expect("protocol")
        .upgrade()
        .await
        .expect("upgrade through the resolve map");
    socket
        .send(WsMessage::Text("from-client".into()))
        .await
        .expect("client text");
    assert_eq!(
        socket.take_next_message().await.expect("fragmented text"),
        WsEvent::Message(WsMessage::Text("hello".into()))
    );
    assert_eq!(
        socket.take_next_message().await.expect("close after ping"),
        WsEvent::Close {
            code: 1000,
            reason: "bye".into(),
        }
    );
    server.assert_clean();
}

#[tokio::test]
async fn wss_dials_use_the_connect_proxy() {
    let proxy = TestServer::start(|connection| {
        let request = connection.read_request();
        assert_eq!(request.method, "CONNECT");
        assert_eq!(request.target, "origin.test:443");
        connection
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .expect("connect denial");
    });
    let Err(error) = AgentBuilder::new()
        .proxy(&format!("http://{}", proxy.local_addr()))
        .expect("proxy")
        .build()
        .request(
            Method::GET,
            url::Url::parse("wss://origin.test/socket").expect("absolute url"),
        )
        .upgrade()
        .await
    else {
        panic!("the proxy denied the tunnel");
    };
    assert!(
        matches!(
            error,
            net::NetError::Transport(net::TransportError::Connect(_))
        ),
        "unexpected proxy error: {error:?}"
    );
    proxy.assert_clean();
}
