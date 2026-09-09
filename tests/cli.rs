use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos()
}

fn temp_dirs() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("tinybrowser-cli-{}", stamp()));
    let runtime = root.join("run");
    let data = root.join("data");
    std::fs::create_dir_all(&runtime).expect("runtime");
    std::fs::create_dir_all(&data).expect("data");
    (runtime, data)
}

fn spawn_daemon(runtime: &Path, data: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(["--daemon"])
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_DATA_HOME", data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("daemon")
}

fn wait_daemon(runtime: &Path) {
    let path = runtime.join("tinybrowser/default/daemon.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::fs::read_to_string(&path).is_err() {
        assert!(Instant::now() < deadline, "daemon missing");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn cli(runtime: &Path, data: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_DATA_HOME", data)
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

#[test]
fn create_list_eval_close_over_cdp() {
    let (runtime, data) = temp_dirs();
    let mut daemon = spawn_daemon(&runtime, &data);
    wait_daemon(&runtime);

    let created = cli(&runtime, &data, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    let listed = cli(&runtime, &data, &["list"]);
    assert!(listed.contains(&created), "{listed}");
    let eval = cli(&runtime, &data, &["eval", "1+2"]).trim().to_owned();
    assert!(eval == "3" || eval == "3.0", "eval={eval}");
    cli(&runtime, &data, &["close", &created]);
    let listed = cli(&runtime, &data, &["list"]);
    assert!(!listed.contains(&created), "{listed}");

    let _ = daemon.kill();
    let _ = daemon.wait();
    let _ = std::fs::remove_dir_all(runtime.parent().expect("root"));
}

fn kill_registered(runtime: &Path) {
    let path = runtime.join("tinybrowser/default/daemon.json");
    if let Ok(text) = std::fs::read_to_string(path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(pid) = value["pid"].as_u64()
    {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
}

fn cli_status(runtime: &Path, data: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_DATA_HOME", data)
        .output()
        .expect("cli")
}

#[test]
fn cli_autostarts_daemon_and_select_navigate_close_last() {
    let (runtime, data) = temp_dirs();
    let created = cli(&runtime, &data, &["create"]).trim().to_owned();
    assert!(!created.is_empty(), "create id");
    wait_daemon(&runtime);

    let other = cli(&runtime, &data, &["create"]).trim().to_owned();
    cli(&runtime, &data, &["select", &created]);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("page bind");
    let page_addr = listener.local_addr().expect("page addr");
    let page_server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
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
    cli(
        &runtime,
        &data,
        &["navigate", &format!("http://{page_addr}/")],
    );
    let eval = cli(
        &runtime,
        &data,
        &[
            "eval",
            "document.getElementsByTagName('p')[0].firstChild.data",
        ],
    )
    .trim()
    .to_owned();
    assert_eq!(eval, "\"nav\"", "eval={eval}");
    page_server.join().expect("page server");

    cli(&runtime, &data, &["close"]);
    let listed = cli(&runtime, &data, &["list"]);
    assert!(!listed.contains(&created), "{listed}");
    assert!(listed.contains(&other), "{listed}");

    cli(&runtime, &data, &["close"]);
    let listed = cli(&runtime, &data, &["list"]);
    assert!(listed.trim().is_empty(), "{listed}");
    let failed = cli_status(&runtime, &data, &["eval", "1"]);
    assert!(!failed.status.success());

    kill_registered(&runtime);
    let _ = std::fs::remove_dir_all(runtime.parent().expect("root"));
}
