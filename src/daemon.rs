//! Named profile daemon: loopback CDP, runtime-dir registration, startup lock.
//!
//! [ADR 0009](../docs/adrs/0009-named-profile-daemon.md)

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use browser::{Browser, Profile, ProfileName, ProfileStore};
use serde_json::{Value, json};

/// Registration recorded at `$XDG_RUNTIME_DIR/tinybrowser/<profile>/daemon.json`.
#[derive(Clone, Debug)]
pub struct DaemonEndpoint {
    /// Process id of the daemon.
    pub pid: u32,
    /// Loopback host, always `127.0.0.1`.
    pub host: String,
    /// Bound port.
    pub port: u16,
}

impl DaemonEndpoint {
    /// Loopback socket for CDP clients.
    #[must_use]
    pub fn addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::from(([127, 0, 0, 1], self.port))
    }
}

/// Runs the profile daemon until the process exits.
///
/// # Errors
///
/// Bind, registration, or serve failure.
pub fn run(profile: &Profile, data_home: &Path) -> io::Result<()> {
    let runtime = profile_runtime_dir(profile.name())?;
    fs::create_dir_all(&runtime)?;
    restrict_dir(&runtime)?;
    let Some(_lock) = acquire_lock(&runtime)? else {
        return Ok(());
    };
    let lock_path = runtime.join("lock");
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    write_endpoint(
        &runtime,
        &DaemonEndpoint {
            pid: std::process::id(),
            host: "127.0.0.1".into(),
            port: addr.port(),
        },
    )?;
    let browser = Browser::open_in(data_home, profile);
    let result = cdp::serve(&listener, &browser.handle());
    let _ = fs::remove_file(&lock_path);
    result
}

/// Returns a live endpoint, starting a detached daemon when missing.
///
/// # Errors
///
/// Spawn or wait failure.
pub fn ensure(profile: &Profile, data_home: &Path) -> io::Result<DaemonEndpoint> {
    let runtime = profile_runtime_dir(profile.name())?;
    if let Some(existing) = read_endpoint(&runtime)
        && pid_alive(existing.pid)
    {
        return Ok(existing);
    }
    spawn_detached(profile, data_home)?;
    wait_endpoint(profile, Duration::from_secs(5))
}

/// Starts the same executable as `--daemon` without attaching stdio.
///
/// # Errors
///
/// Spawn failure.
pub fn spawn_detached(profile: &Profile, data_home: &Path) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command
        .arg("--daemon")
        .arg(format!("--profile={}", profile.name().as_str()))
        .env("XDG_DATA_HOME", data_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(runtime) = env_runtime_dir() {
        command.env("XDG_RUNTIME_DIR", runtime);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()?;
    Ok(())
}

fn wait_endpoint(profile: &Profile, timeout: Duration) -> io::Result<DaemonEndpoint> {
    let runtime = profile_runtime_dir(profile.name())?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(existing) = read_endpoint(&runtime)
            && pid_alive(existing.pid)
        {
            return Ok(existing);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "profile daemon did not start",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn profile_runtime_dir(name: &ProfileName) -> io::Result<PathBuf> {
    Ok(runtime_home()?.join("tinybrowser").join(name.as_str()))
}

fn runtime_home() -> io::Result<PathBuf> {
    if let Some(dir) = env_runtime_dir() {
        return Ok(dir);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "XDG_RUNTIME_DIR is unset",
    ))
}

fn env_runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn acquire_lock(runtime: &Path) -> io::Result<Option<File>> {
    let path = runtime.join("lock");
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id())?;
                file.sync_all()?;
                restrict_file(&path)?;
                return Ok(Some(file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match lock_status(&path) {
                    LockStatus::Held => return Ok(None),
                    LockStatus::InProgress => std::thread::sleep(Duration::from_millis(20)),
                    LockStatus::Stale => {
                        if fs::remove_file(&path).is_err() {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
}

enum LockStatus {
    Held,
    InProgress,
    Stale,
}

fn lock_status(path: &Path) -> LockStatus {
    let Ok(text) = fs::read_to_string(path) else {
        return if lock_file_stale(path) {
            LockStatus::Stale
        } else {
            LockStatus::InProgress
        };
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return if lock_file_stale(path) {
            LockStatus::Stale
        } else {
            LockStatus::InProgress
        };
    }
    let Ok(pid) = trimmed.parse::<u32>() else {
        return if lock_file_stale(path) {
            LockStatus::Stale
        } else {
            LockStatus::InProgress
        };
    };
    if pid <= 1 {
        return LockStatus::Stale;
    }
    if pid_alive(pid) {
        LockStatus::Held
    } else {
        LockStatus::Stale
    }
}

fn lock_file_stale(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|meta| meta.modified()) else {
        return true;
    };
    SystemTime::now()
        .duration_since(modified)
        .map_or(true, |age| age > Duration::from_secs(2))
}

fn restrict_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn restrict_file(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn read_endpoint(runtime: &Path) -> Option<DaemonEndpoint> {
    let path = runtime.join("daemon.json");
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let pid = value.get("pid").and_then(Value::as_u64)?;
    let port = value.get("port").and_then(Value::as_u64)?;
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1");
    if host != "127.0.0.1" {
        return None;
    }
    let pid = u32::try_from(pid).ok()?;
    let port = u16::try_from(port).ok()?;
    if pid <= 1 || port == 0 {
        return None;
    }
    Some(DaemonEndpoint {
        pid,
        host: host.to_owned(),
        port,
    })
}

fn write_endpoint(runtime: &Path, endpoint: &DaemonEndpoint) -> io::Result<()> {
    let path = runtime.join("daemon.json");
    let tmp = runtime.join("daemon.json.tmp");
    let body = json!({
        "pid": endpoint.pid,
        "host": endpoint.host,
        "port": endpoint.port,
    });
    fs::write(&tmp, body.to_string())?;
    restrict_file(&tmp)?;
    fs::rename(&tmp, &path)?;
    restrict_file(&path)
}

fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Data home for profile files: `XDG_DATA_HOME` or `$HOME/.local/share`.
///
/// # Errors
///
/// Both `XDG_DATA_HOME` and `HOME` are unset or empty.
pub fn data_home() -> io::Result<PathBuf> {
    ProfileStore::data_home()
}

/// Selected target id for the CLI convenience.
///
/// # Errors
///
/// Runtime dir missing or write failure.
pub fn write_selected(profile: &Profile, target: &str) -> io::Result<()> {
    let runtime = profile_runtime_dir(profile.name())?;
    fs::create_dir_all(&runtime)?;
    restrict_dir(&runtime)?;
    let path = runtime.join("selected");
    fs::write(&path, target)?;
    restrict_file(&path)
}

/// Drops the last selected target id.
///
/// # Errors
///
/// Runtime dir missing.
pub fn clear_selected(profile: &Profile) -> io::Result<()> {
    let runtime = profile_runtime_dir(profile.name())?;
    match fs::remove_file(runtime.join("selected")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Last selected target, if any.
///
/// # Errors
///
/// Runtime dir missing.
pub fn read_selected(profile: &Profile) -> io::Result<Option<String>> {
    let runtime = profile_runtime_dir(profile.name())?;
    match fs::read_to_string(runtime.join("selected")) {
        Ok(text) => Ok(Some(text.trim().to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
