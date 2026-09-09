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
