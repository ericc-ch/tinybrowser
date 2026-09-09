use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Fixture {
    pub runtime: PathBuf,
    pub data: PathBuf,
    pub children: Vec<Child>,
}

impl Fixture {
    pub fn new(prefix: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("{prefix}-{stamp}"));
        let runtime = root.join("run");
        let data = root.join("data");
        std::fs::create_dir_all(&runtime).expect("runtime");
        std::fs::create_dir_all(&data).expect("data");
        Self {
            runtime,
            data,
            children: Vec::new(),
        }
    }

    pub fn spawn_daemon(&mut self) -> u32 {
        let child = Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
            .args(["--daemon", "--profile=default"])
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", &self.data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");
        let id = child.id();
        self.children.push(child);
        id
    }

    pub fn wait_json(&self) -> serde_json::Value {
        let path = self.runtime.join("tinybrowser/default/daemon.json");
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(text) =
            std::fs::read_to_string(self.runtime.join("tinybrowser/default/daemon.json"))
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(pid) = value["pid"].as_u64()
        {
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if let Some(root) = self.runtime.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
