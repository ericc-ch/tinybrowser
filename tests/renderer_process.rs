//! Renderer-process lifecycle over CDP: one daemon, one renderer child per site.

mod common;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use common::Fixture;
use serde_json::{Value, json};

fn attach(client: &mut cdp::Client, target: &str) -> String {
    let attached = client
        .call(
            "Target.attachToTarget",
            &json!({"targetId": target, "flatten": true}),
            None,
        )
        .expect("attach");
    attached["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_owned()
}

fn create(client: &mut cdp::Client) -> String {
    let result = client
        .call("Target.createTarget", &json!({"url": "about:blank"}), None)
        .expect("createTarget");
    result["targetId"].as_str().expect("targetId").to_owned()
}

fn close(client: &mut cdp::Client, target: &str) {
    client
        .call("Target.closeTarget", &json!({"targetId": target}), None)
        .expect("closeTarget");
}

fn eval(client: &mut cdp::Client, target: &str, script: &str) -> Result<String, String> {
    let session = attach(client, target);
    let result = client
        .call(
            "Runtime.evaluate",
            &json!({"expression": script}),
            Some(&session),
        )
        .map_err(|error| error.to_string())?;
    if let Some(text) = result
        .pointer("/exceptionDetails/text")
        .and_then(Value::as_str)
    {
        return Err(text.to_owned());
    }
    let preview = result.get("result").cloned().unwrap_or(Value::Null);
    Ok(match preview.get("value") {
        Some(value) => value.to_string(),
        None => preview
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("undefined")
            .to_owned(),
    })
}

fn navigate(client: &mut cdp::Client, target: &str, url: &str) {
    let session = attach(client, target);
    client
        .call("Page.enable", &json!({}), Some(&session))
        .expect("Page.enable");
    client
        .call("Page.navigate", &json!({"url": url}), Some(&session))
        .expect("Page.navigate");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("navigation did not finish within 30 seconds");
        let Some(event) = client.read_event(remaining).expect("read event") else {
            panic!("navigation did not finish within 30 seconds");
        };
        if event.get("method").and_then(Value::as_str) == Some("Page.loadEventFired")
            && event.get("sessionId").and_then(Value::as_str) == Some(&session)
        {
            break;
        }
    }
    client
        .call("Page.disable", &json!({}), Some(&session))
        .expect("Page.disable");
}

fn daemon_pid(fixture: &Fixture) -> u32 {
    u32::try_from(fixture.wait_json()["pid"].as_u64().expect("daemon pid")).expect("pid")
}

/// `--renderer` children of `daemon`, found through `/proc` (Linux).
fn renderer_children(daemon: u32) -> Vec<u32> {
    let mut children = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return children;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if !cmdline
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--renderer")
        {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, after_comm)) = stat.rsplit_once(')') else {
            continue;
        };
        if after_comm
            .split_whitespace()
            .nth(1)
            .and_then(|ppid| ppid.parse::<u32>().ok())
            == Some(daemon)
        {
            children.push(pid);
        }
    }
    children
}

fn wait_for_renderers(daemon: u32, expect_empty: bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let children = renderer_children(daemon);
        if children.is_empty() == expect_empty {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "renderer children still {}: {children:?}",
            if expect_empty { "present" } else { "missing" }
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn(fixture: &mut Fixture) -> (cdp::Client, String) {
    fixture.spawn_daemon();
    let _ = fixture.wait_json();
    let mut client = fixture.connect();
    let created = create(&mut client);
    assert!(!created.is_empty(), "create id");
    (client, created)
}

#[test]
fn renderer_process_runs_classic_scripts_and_fetches() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("server bind");
    let addr = listener.local_addr().expect("server addr");
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut head = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("read");
                assert_ne!(read, 0, "peer closed before request head");
                head.extend_from_slice(&chunk[..read]);
            }
            let target = String::from_utf8_lossy(&head)
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .expect("request target")
                .to_owned();
            let (content_type, body): (&str, &[u8]) = match target.as_str() {
                "/page" => (
                    "text/html",
                    b"<!doctype html><script src=\"/lib.js\"></script><script>window.ready = false; fetch('/data').then(function(r){return r.text();}).then(function(t){window.payload = t; window.ready = true;});</script>",
                ),
                "/lib.js" => ("text/javascript", b"window.fromLib = 7;"),
                "/data" => ("text/plain", b"payload"),
                other => panic!("unexpected request {other}"),
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("head");
            stream.write_all(body).expect("body");
        }
    });

    let mut fixture = Fixture::new("tinybrowser-renderer");
    let (mut client, created) = spawn(&mut fixture);
    let daemon = daemon_pid(&fixture);
    assert!(
        !renderer_children(daemon).is_empty(),
        "create must spawn a --renderer child of the daemon"
    );
    navigate(&mut client, &created, &format!("http://{addr}/page"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if eval(&mut client, &created, "window.fromLib").unwrap() == "7.0" {
            break;
        }
        assert!(Instant::now() < deadline, "classic script never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if eval(&mut client, &created, "window.ready === true").unwrap() == "true" {
            break;
        }
        assert!(Instant::now() < deadline, "js fetch never completed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        eval(&mut client, &created, "window.payload").unwrap(),
        "\"payload\"",
        "js fetch must cross the renderer pipe"
    );
    close(&mut client, &created);
    server.join().expect("server");
}

#[test]
fn renderer_process_survives_non_finite_results() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    let (mut client, created) = spawn(&mut fixture);

    assert_eq!(
        eval(&mut client, &created, "1/0").unwrap(),
        "null",
        "non-finite number crosses as JSON null"
    );
    // The renderer must still answer after a non-finite result.
    assert_eq!(eval(&mut client, &created, "2+2").unwrap(), "4.0");
    close(&mut client, &created);
}

#[test]
fn close_interrupts_a_running_script_in_the_renderer_process() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    let (mut client, created) = spawn(&mut fixture);

    let endpoint = fixture.endpoint();
    let target = created.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        let mut runner = cdp::Client::connect(endpoint).expect("runner connect");
        let result = eval(&mut runner, &target, "while(true){}");
        let _ = done_tx.send(result);
    });
    std::thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    close(&mut client, &created);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close waited on the running script: {:?}",
        started.elapsed()
    );
    let _ = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("running eval never returned after close");
    runner.join().expect("runner");
}

#[test]
fn closing_an_opaque_page_reaps_its_renderer_process() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    let (mut client, created) = spawn(&mut fixture);
    let daemon = daemon_pid(&fixture);
    wait_for_renderers(daemon, false, Duration::from_secs(5));

    close(&mut client, &created);
    wait_for_renderers(daemon, true, Duration::from_secs(5));
}

#[test]
fn log_level_reaches_the_daemon_file() {
    let mut fixture = Fixture::new("tinybrowser-log-file");
    fixture.spawn_daemon_with_env("TINYBROWSER_LOG", "debug");
    let _ = fixture.wait_json();
    let mut client = fixture.connect();
    let created = create(&mut client);

    let path = fixture.data.join("tinybrowser/logs/default.log");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut text = String::new();
    loop {
        if let Ok(read) = std::fs::read_to_string(&path)
            && read.contains("process=daemon")
            && read.contains("level=DEBUG")
            && read.contains("process=renderer")
        {
            text = read;
            break;
        }
        assert!(
            Instant::now() < deadline,
            "incomplete daemon log at {}: {text}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        text.contains("message=ready"),
        "renderer ready record missing"
    );
    close(&mut client, &created);
}
