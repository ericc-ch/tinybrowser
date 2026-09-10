mod common;

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::Fixture;

fn cli(fixture: &Fixture, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .output()
        .expect("cli");
    assert!(
        output.status.success(),
        "cli {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn cli_status(fixture: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .output()
        .expect("cli")
}

/// Runs the CLI with a wall-clock bound so a regression fails instead of
/// hanging the suite.
fn cli_until(fixture: &Fixture, args: &[&str], timeout: Duration) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cli");
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("wait").is_some() {
            return child.wait_with_output().expect("output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("output");
            panic!(
                "cli {args:?} timed out: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
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

#[test]
fn create_list_eval_close_over_cdp() {
    let mut fixture = Fixture::new("tinybrowser-cli");
    fixture.spawn_daemon();
    let _ = fixture.wait_json();

    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let listed = cli(&fixture, &["list"]);
    assert!(listed.contains(&created), "{listed}");
    let eval = cli(&fixture, &["eval", "1+2"]).trim().to_owned();
    assert!(eval == "3" || eval == "3.0", "eval={eval}");
    cli(&fixture, &["close", &created]);
    let listed = cli(&fixture, &["list"]);
    assert!(!listed.contains(&created), "{listed}");
}

#[test]
fn renderer_process_runs_classic_scripts_and_fetches() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("page bind");
    let addr = listener.local_addr().expect("page addr");
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

    let fixture = Fixture::new("tinybrowser-cli");
    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let _ = fixture.wait_json();
    assert!(
        !renderer_children(daemon_pid(&fixture)).is_empty(),
        "create must spawn a --renderer child of the daemon"
    );
    cli(&fixture, &["navigate", &format!("http://{addr}/page")]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if cli(&fixture, &["eval", "window.fromLib"]).trim() == "7.0" {
            break;
        }
        assert!(Instant::now() < deadline, "classic script never ran");
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if cli(&fixture, &["eval", "window.ready === true"]).trim() == "true" {
            break;
        }
        assert!(Instant::now() < deadline, "js fetch never completed");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        cli(&fixture, &["eval", "window.payload"]).trim(),
        "\"payload\"",
        "js fetch must cross the renderer pipe"
    );
    cli(&fixture, &["close"]);
    server.join().expect("page server");
}

#[test]
fn renderer_process_survives_non_finite_results() {
    let fixture = Fixture::new("tinybrowser-cli");
    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let _ = fixture.wait_json();

    let infinity = cli_until(&fixture, &["eval", "1/0"], Duration::from_secs(10));
    assert!(
        infinity.status.success(),
        "infinity eval failed: {}",
        String::from_utf8_lossy(&infinity.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&infinity.stdout).trim(),
        "null",
        "non-finite number crosses as JSON null"
    );
    // The renderer must still answer after a non-finite result.
    assert_eq!(cli(&fixture, &["eval", "2+2"]).trim(), "4.0");
    cli(&fixture, &["close", &created]);
}

#[test]
fn close_interrupts_a_running_script_in_the_renderer_process() {
    let fixture = Fixture::new("tinybrowser-cli");
    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let _ = fixture.wait_json();

    let mut running = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(["eval", "while(true){}"])
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("eval cli");
    std::thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    let closed = cli_until(&fixture, &["close", &created], Duration::from_secs(8));
    assert!(
        closed.status.success(),
        "close failed: {}",
        String::from_utf8_lossy(&closed.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "close waited on the running script: {:?}",
        started.elapsed()
    );
    let _ = running.kill();
    let _ = running.wait();
}

#[test]
fn closing_an_opaque_page_reaps_its_renderer_process() {
    let fixture = Fixture::new("tinybrowser-cli");
    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let _ = fixture.wait_json();
    let daemon = daemon_pid(&fixture);
    wait_for_renderers(daemon, false, Duration::from_secs(5));

    cli(&fixture, &["close", &created]);
    wait_for_renderers(daemon, true, Duration::from_secs(5));
}

#[test]
fn daemon_rejects_webdriver_and_resolve() {
    let webdriver = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(["--daemon", "--webdriver=9"])
        .stdin(Stdio::null())
        .output()
        .expect("cli");
    assert!(!webdriver.status.success());
    let err = String::from_utf8_lossy(&webdriver.stderr);
    assert!(
        err.contains("--daemon and --webdriver are mutually exclusive"),
        "{err}"
    );

    let resolve = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(["--daemon", "--resolve=*.test=127.0.0.1"])
        .stdin(Stdio::null())
        .output()
        .expect("cli");
    assert!(!resolve.status.success());
    let err = String::from_utf8_lossy(&resolve.stderr);
    assert!(
        err.contains("--daemon and --resolve are mutually exclusive"),
        "{err}"
    );

    let command = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(["--daemon", "create"])
        .stdin(Stdio::null())
        .output()
        .expect("cli");
    assert!(!command.status.success());
    let err = String::from_utf8_lossy(&command.stderr);
    assert!(err.contains("--daemon does not accept a command"), "{err}");
}

#[test]
fn cli_autostarts_daemon_and_select_navigate_close_last() {
    let fixture = Fixture::new("tinybrowser-cli");
    let created = cli(&fixture, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let _ = fixture.wait_json();

    let other = cli(&fixture, &["create"]).trim().to_owned();
    cli(&fixture, &["select", &created]);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("page bind");
    listener
        .set_nonblocking(true)
        .expect("accept must not hang the test");
    let page_addr = listener.local_addr().expect("page addr");
    let page_server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "CLI never connected");
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        stream.set_nonblocking(false).expect("blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut buf = [0_u8; 512];
        let _ = stream.read(&mut buf);
        let body = b"<!doctype html><p id=ok>nav</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("head");
        stream.write_all(body).expect("body");
    });
    cli(&fixture, &["navigate", &format!("http://{page_addr}/")]);
    let eval = cli(
        &fixture,
        &[
            "eval",
            "document.getElementsByTagName('p')[0].firstChild.data",
        ],
    )
    .trim()
    .to_owned();
    assert_eq!(eval, "\"nav\"", "eval={eval}");
    page_server.join().expect("page server");

    cli(&fixture, &["close"]);
    let listed = cli(&fixture, &["list"]);
    assert!(!listed.contains(&created), "{listed}");
    assert!(listed.contains(&other), "{listed}");

    cli(&fixture, &["close"]);
    let listed = cli(&fixture, &["list"]);
    assert!(listed.trim().is_empty(), "{listed}");
    let failed = cli_status(&fixture, &["eval", "1"]);
    assert!(!failed.status.success());
}
