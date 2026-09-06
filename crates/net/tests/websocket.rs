mod common;

use std::sync::{Arc, Mutex};

use common::TestServer;
use net::{AgentBuilder, Method, WsEvent, WsMessage};
use tungstenite::protocol::frame::Frame;
use tungstenite::protocol::frame::coding::{CloseCode, Data, OpCode};
use tungstenite::{Message, accept_hdr};

struct HandshakeCapture {
    slot: Arc<Mutex<Option<(String, String)>>>,
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
        *self.slot.lock().expect("capture") = Some((origin, user_agent));
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
    captured: Arc<Mutex<Option<(String, String)>>>,
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

#[test]
fn websocket_transcript_covers_handshake_frames_control_and_cookie_reuse() {
    let captured = Arc::new(Mutex::new(None));
    let requests = Arc::new(Mutex::new(0_u8));
    let server = start_websocket_server(Arc::clone(&captured), Arc::clone(&requests));

    let agent = AgentBuilder::new().user_agent("tinybrowser-test/1").build();
    let document =
        url::Url::parse(&format!("ws://{}/page", server.local_addr())).expect("document");
    let socket = agent
        .request(Method::GET, server.ws_url("/socket"))
        .with_initiator(document)
        .upgrade()
        .expect("upgrade");
    socket
        .send(WsMessage::Text("from-client".into()))
        .expect("client text");
    assert_eq!(
        socket.take_next_message().expect("fragmented text"),
        WsEvent::Message(WsMessage::Text("hello".into()))
    );
    assert_eq!(
        socket.take_next_message().expect("close after ping"),
        WsEvent::Close {
            code: 1000,
            reason: "bye".into(),
        }
    );

    let (origin, user_agent) = captured
        .lock()
        .expect("capture")
        .clone()
        .expect("captured handshake");
    assert_eq!(origin, format!("ws://{}", server.local_addr()));
    assert_eq!(user_agent, "tinybrowser-test/1");

    agent
        .request(Method::GET, server.url("/after"))
        .send()
        .expect("cookie follow-up");
    assert_eq!(*requests.lock().expect("request count"), 2);
    server.assert_clean();
}
