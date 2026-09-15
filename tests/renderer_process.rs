//! Renderer-process lifecycle over CDP: one daemon, one renderer child per site.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
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
            &json!({"expression": script, "returnByValue": true}),
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

fn daemon_pid(fixture: &Fixture) -> u32 {
    u32::try_from(fixture.wait_json()["pid"].as_u64().expect("daemon pid")).expect("pid")
}

/// `renderer` children of `daemon`, found through `/proc` (Linux).
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
            .any(|arg| arg == b"renderer")
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

fn wait_for_renderer_count(daemon: u32, expected: usize, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if renderer_children(daemon).len() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "renderer count never reached {expected}: {:?}",
            renderer_children(daemon)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn target_ids(client: &mut cdp::Client) -> Vec<String> {
    client
        .call("Target.getTargets", &json!({}), None)
        .expect("targets")["targetInfos"]
        .as_array()
        .expect("targetInfos")
        .iter()
        .map(|info| info["targetId"].as_str().expect("targetId").to_owned())
        .collect()
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
fn renderer_process_survives_non_finite_results() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    let (mut client, created) = spawn(&mut fixture);
    let session = attach(&mut client, &created);

    let infinity = client
        .call(
            "Runtime.evaluate",
            &json!({"expression": "1/0", "returnByValue": true}),
            Some(&session),
        )
        .expect("infinity");
    assert_eq!(infinity["result"]["unserializableValue"], json!("Infinity"));
    assert!(infinity["result"].get("value").is_none());

    // The renderer must still answer after a non-finite result.
    assert_eq!(eval(&mut client, &created, "2+2").unwrap(), "4");
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
fn blank_targets_stay_virtual_and_idle_processes_collapse_to_one_spare() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    fixture.spawn_daemon();
    let _ = fixture.wait_json();
    let mut client = fixture.connect();
    let daemon = daemon_pid(&fixture);
    wait_for_renderers(daemon, false, Duration::from_secs(5));
    let initial_targets = target_ids(&mut client);
    assert_eq!(
        initial_targets.len(),
        1,
        "daemon starts with one initial tab"
    );
    let before = renderer_children(daemon).len();

    let created = create(&mut client);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        renderer_children(daemon).len(),
        before,
        "creating a blank target must not consume the spare"
    );

    assert_eq!(eval(&mut client, &created, "2+2").unwrap(), "4");
    wait_for_renderer_count(daemon, before + 1, Duration::from_secs(5));
    close(&mut client, &created);
    wait_for_renderer_count(daemon, before, Duration::from_secs(5));

    // Closing the virtual initial tab leaves the manager's one unlocked spare.
    close(&mut client, &initial_targets[0]);
    wait_for_renderer_count(daemon, before, Duration::from_secs(5));
}

#[test]
fn renderers_exit_when_the_daemon_is_killed() {
    let mut fixture = Fixture::new("tinybrowser-renderer");
    fixture.spawn_daemon();
    let _ = fixture.wait_json();
    let daemon = daemon_pid(&fixture);
    wait_for_renderers(daemon, false, Duration::from_secs(5));
    let renderers = renderer_children(daemon);
    assert!(!renderers.is_empty(), "initial tab must have a renderer");

    let killed = std::process::Command::new("kill")
        .args(["-9", &daemon.to_string()])
        .status()
        .expect("kill");
    assert!(killed.success(), "SIGKILL daemon");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let alive: Vec<u32> = renderers
            .iter()
            .copied()
            .filter(|pid| pid_alive(*pid))
            .collect();
        if alive.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "renderers outlived their daemon: {alive:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
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

#[test]
fn same_site_assignments_share_only_after_the_process_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).expect("read request");
            let body = b"<!doctype html><title>shared-site</title>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("write head");
            stream.write_all(body).expect("write body");
        }
    });

    let mut fixture = Fixture::new("tinybrowser-renderer-budget");
    fixture.spawn_daemon_with_env("TINYBROWSER_RENDERER_PROCESS_LIMIT", "1");
    let _ = fixture.wait_json();
    let daemon = daemon_pid(&fixture);
    wait_for_renderer_count(daemon, 1, Duration::from_secs(5));
    let mut client = fixture.connect();
    let first = create(&mut client);
    let second = create(&mut client);
    let first_session = attach(&mut client, &first);
    let second_session = attach(&mut client, &second);
    for session in [&first_session, &second_session] {
        client
            .call("Page.enable", &json!({}), Some(session))
            .expect("enable");
        client
            .call(
                "Page.navigate",
                &json!({"url": format!("http://{address}/")}),
                Some(session),
            )
            .expect("navigate");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut loaded = Vec::new();
    while loaded.len() < 2 {
        let event = client
            .read_event(Duration::from_millis(500))
            .expect("event read")
            .expect("event");
        if event["method"] == json!("Page.loadEventFired") {
            loaded.push(event["sessionId"].as_str().expect("session").to_owned());
        }
        assert!(Instant::now() < deadline, "both assignments did not load");
    }
    assert_eq!(renderer_children(daemon).len(), 1);
    assert_eq!(
        eval(&mut client, &first, "window.marker = 41").unwrap(),
        "41"
    );
    assert_eq!(
        eval(&mut client, &second, "typeof window.marker").unwrap(),
        "\"undefined\""
    );
    server.join().expect("server");
}
