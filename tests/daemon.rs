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
    let root = std::env::temp_dir().join(format!("tinybrowser-daemon-{}", stamp()));
    let runtime = root.join("run");
    let data = root.join("data");
    std::fs::create_dir_all(&runtime).expect("runtime");
    std::fs::create_dir_all(&data).expect("data");
    (runtime, data)
}

fn spawn_daemon(runtime: &Path, data: &Path) -> Child {
    let exe = env!("CARGO_BIN_EXE_tinybrowser");
    Command::new(exe)
        .args(["--daemon", "--profile=default"])
        .env("XDG_RUNTIME_DIR", runtime)
        .env("XDG_DATA_HOME", data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon")
}

fn wait_json(runtime: &Path) -> serde_json::Value {
    let path = runtime.join("tinybrowser/default/daemon.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        {
            return value;
        }
        assert!(Instant::now() < deadline, "daemon.json missing");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_alive(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
}

#[test]
fn daemon_registers_loopback_and_second_start_reuses_it() {
    let (runtime, data) = temp_dirs();
    let mut first = spawn_daemon(&runtime, &data);
    let info = wait_json(&runtime);
    let pid = u32::try_from(info["pid"].as_u64().expect("pid")).expect("pid u32");
    let port = u16::try_from(info["port"].as_u64().expect("port")).expect("port u16");
    assert_eq!(info["host"], "127.0.0.1");
    assert!(pid_alive(pid));

    let mut client = cdp::Client::connect(format!("127.0.0.1:{port}").parse().expect("addr"))
        .expect("cdp connect");
    let version = client
        .call("Browser.getVersion", &serde_json::json!({}), None)
        .expect("version");
    assert_eq!(version["product"], serde_json::json!("tinybrowser/0.1.0"));

    let mut second = spawn_daemon(&runtime, &data);
    let again = wait_json(&runtime);
    assert_eq!(again["pid"], info["pid"]);
    assert_eq!(again["port"], info["port"]);
    let _ = second.wait();

    let _ = first.kill();
    let _ = first.wait();
    let _ = std::fs::remove_dir_all(runtime.parent().expect("root"));
}

#[test]
fn stale_lock_is_replaced() {
    let (runtime, data) = temp_dirs();
    let dir = runtime.join("tinybrowser/default");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("lock"), "999999\n").expect("stale lock");
    std::fs::write(
        dir.join("daemon.json"),
        r#"{"pid":999999,"host":"127.0.0.1","port":1}"#,
    )
    .expect("stale json");
    let mut child = spawn_daemon(&runtime, &data);
    let deadline = Instant::now() + Duration::from_secs(5);
    let info = loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("daemon.json"))
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && value["pid"].as_u64() != Some(999_999)
            && pid_alive(u32::try_from(value["pid"].as_u64().unwrap_or(0)).unwrap_or(0))
        {
            break value;
        }
        if !pid_alive(child.id()) {
            let _ = child.try_wait();
            panic!("daemon exited before replacing stale registration");
        }
        assert!(
            Instant::now() < deadline,
            "stale daemon.json not replaced: {:?}",
            std::fs::read_to_string(dir.join("daemon.json"))
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(info["port"].as_u64().expect("port") > 1);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(runtime.parent().expect("root"));
}

#[test]
fn overlapping_daemons_share_one_endpoint() {
    let (runtime, data) = temp_dirs();
    let mut first = spawn_daemon(&runtime, &data);
    let mut second = spawn_daemon(&runtime, &data);
    let info = wait_json(&runtime);
    let pid = u32::try_from(info["pid"].as_u64().expect("pid")).expect("pid u32");
    assert!(pid_alive(pid));
    let again = wait_json(&runtime);
    assert_eq!(again["pid"], info["pid"]);
    assert_eq!(again["port"], info["port"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let first_dead = first.try_wait().expect("first wait").is_some();
        let second_dead = second.try_wait().expect("second wait").is_some();
        if first_dead ^ second_dead {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "one overlapping daemon must exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(pid_alive(pid));
    let _ = first.kill();
    let _ = first.wait();
    let _ = second.kill();
    let _ = second.wait();
    let _ = std::fs::remove_dir_all(runtime.parent().expect("root"));
}
