//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! [ADR 0010](../../../docs/adrs/0010-page-actor-ownership.md): page actors
//! receive [`FetchHandle`]. They do not expose or own [`net::Agent`].

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use net::{Agent, AgentBuilder, CookieRecord, CookieSameSite, Method};
use url::Url;

use crate::profile::{Profile, ProfileName};

/// Default per-call fetch timeout for a page-owned or browser-owned agent.
pub(crate) const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

const COOKIES_VERSION: &str = "tinybrowser-cookies-v1";
const MAX_BROWSER_DIALS: usize = 16;
const MAX_QUEUED_DIALS: usize = 256;
static COOKIE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Durable backing for one Profile under `XDG_DATA_HOME`.
pub struct ProfileStore {
    name: ProfileName,
    root: Option<PathBuf>,
    _lock: Option<File>,
    disk: Mutex<()>,
    dirty: AtomicBool,
}

impl ProfileStore {
    /// Data home for profile files: `XDG_DATA_HOME` or `$HOME/.local/share`.
    ///
    /// # Errors
    ///
    /// Both `XDG_DATA_HOME` and `HOME` are unset or empty.
    pub fn data_home() -> io::Result<PathBuf> {
        if let Some(dir) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(dir));
        }
        if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(home).join(".local/share"));
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "XDG_DATA_HOME and HOME are unset",
        ))
    }

    /// Opens the on-disk store for `profile` under the process XDG data home.
    ///
    /// # Errors
    ///
    /// Both `XDG_DATA_HOME` and `HOME` are unset or empty.
    pub fn open(profile: &Profile) -> io::Result<Self> {
        Self::open_in(&Self::data_home()?, profile)
    }

    /// Opens and exclusively locks the on-disk store for `profile` under `data_home`.
    ///
    /// # Errors
    ///
    /// The directory cannot be created or another process owns the profile.
    pub fn open_in(data_home: &Path, profile: &Profile) -> io::Result<Self> {
        let root = data_home
            .join("tinybrowser")
            .join("profiles")
            .join(profile.name().as_str());
        fs::create_dir_all(&root)?;
        restrict_dir(&root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("lock"))?;
        lock.try_lock().map_err(io::Error::from)?;
        Ok(Self {
            name: profile.name().clone(),
            root: Some(root),
            _lock: Some(lock),
            disk: Mutex::new(()),
            dirty: AtomicBool::new(false),
        })
    }

    /// In-memory store: [`ProfileStore::save_from`] is a no-op.
    #[must_use]
    pub fn memory(profile: &Profile) -> Self {
        Self {
            name: profile.name().clone(),
            root: None,
            _lock: None,
            disk: Mutex::new(()),
            dirty: AtomicBool::new(false),
        }
    }

    pub(crate) fn load_into(&self, agent: &Agent) -> io::Result<()> {
        let Some(path) = self.cookies_path() else {
            return Ok(());
        };
        let _disk = self.lock_disk();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let records = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| parse_cookies(text).ok());
        if let Some(records) = records {
            agent.import_cookies(records);
            return Ok(());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        fs::rename(
            &path,
            path.with_file_name(format!("cookies.corrupt.{stamp}")),
        )?;
        Ok(())
    }

    pub(crate) fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    pub(crate) fn save_from(&self, agent: &Agent) -> io::Result<()> {
        let Some(dir) = self.root.as_ref() else {
            return Ok(());
        };
        let _disk = self.lock_disk();
        if !self.dirty.load(Ordering::SeqCst) {
            return Ok(());
        }
        fs::create_dir_all(dir)?;
        restrict_dir(dir)?;
        let path = dir.join("cookies");
        let seq = COOKIE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!("cookies.{}.{seq}.tmp", std::process::id()));
        let encoded = encode_cookies(&agent.export_cookies());
        let write_result = (|| {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            restrict_file(&tmp)?;
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp, &path)?;
            restrict_file(&path)?;
            File::open(dir)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _remove_result = fs::remove_file(&tmp);
            return Err(error);
        }
        self.dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn lock_disk(&self) -> std::sync::MutexGuard<'_, ()> {
        self.disk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cookies_path(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|dir| dir.join("cookies"))
    }

    /// Profile this store belongs to.
    #[must_use]
    pub fn profile_name(&self) -> &ProfileName {
        &self.name
    }
}

/// Browser-owned live networking service for one Profile.
///
/// Wraps one shared [`Agent`]: connection pool, transport settings, and the
/// live cookie jar. [`ProfileStore`] is durable backing, not a second jar.
pub struct NetworkSession {
    agent: Agent,
    store: Arc<ProfileStore>,
    executor: NetworkExecutor,
}

impl NetworkSession {
    /// Session from `builder`, with the default per-call fetch timeout.
    ///
    /// # Errors
    ///
    /// Stored profile data could not be read or quarantined.
    pub fn from_builder(builder: AgentBuilder, store: ProfileStore) -> io::Result<Self> {
        Self::from_agent(builder.timeout_per_call(PAGE_FETCH_TIMEOUT).build(), store)
    }

    /// Session that shares `agent` (and therefore its cookie jar).
    ///
    /// # Errors
    ///
    /// Stored profile data could not be read or quarantined.
    pub fn from_agent(agent: Agent, store: ProfileStore) -> io::Result<Self> {
        store.load_into(&agent)?;
        Ok(Self {
            agent,
            store: Arc::new(store),
            executor: NetworkExecutor::new(),
        })
    }

    /// Value-only fetch handle for a page actor.
    #[must_use]
    pub fn fetch_handle(&self) -> FetchHandle {
        FetchHandle {
            agent: self.agent.clone(),
            store: Arc::clone(&self.store),
            executor: self.executor.clone(),
        }
    }

    pub(crate) fn persist(&self) -> io::Result<()> {
        self.store.save_from(&self.agent)
    }

    pub(crate) fn profile_name(&self) -> ProfileName {
        self.store.profile_name().clone()
    }
}

impl Drop for NetworkSession {
    fn drop(&mut self) {
        let _result = self.persist();
    }
}

/// Cloneable, sendable handle for cookies and blocking HTTP.
///
/// Completions return to the page actor from the bounded network executor.
#[derive(Clone)]
pub struct FetchHandle {
    agent: Agent,
    store: Arc<ProfileStore>,
    executor: NetworkExecutor,
}

impl FetchHandle {
    pub(crate) fn from_agent(agent: Agent) -> Self {
        Self {
            agent,
            store: Arc::new(ProfileStore::memory(&Profile::default())),
            executor: NetworkExecutor::new(),
        }
    }

    /// `document.cookie` getter for `url`.
    #[must_use]
    pub fn cookies_for(&self, url: &Url) -> String {
        self.agent.cookies_for(url)
    }

    /// `document.cookie` setter for `url`.
    pub fn set_cookie(&self, value: &str, url: &Url) {
        self.agent.set_cookie(value, url);
        self.store.mark_dirty();
    }

    pub(crate) fn mark_dirty(&self) {
        self.store.mark_dirty();
    }

    pub(crate) fn request(&self, method: Method, url: Url) -> net::RequestBuilder {
        self.agent.request(method, url)
    }

    pub(crate) fn try_submit(&self, operation: impl FnOnce() + Send + 'static) -> Result<(), ()> {
        self.executor.try_submit(operation)
    }
}

type NetworkJob = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone)]
struct NetworkExecutor {
    tx: SyncSender<NetworkJob>,
}

impl NetworkExecutor {
    fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<NetworkJob>(MAX_QUEUED_DIALS);
        let receiver = Arc::new(Mutex::new(rx));
        for _ in 0..MAX_BROWSER_DIALS {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || {
                loop {
                    let job = receiver
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv();
                    let Ok(job) = job else {
                        return;
                    };
                    job();
                }
            });
        }
        Self { tx }
    }

    fn try_submit(&self, operation: impl FnOnce() + Send + 'static) -> Result<(), ()> {
        match self.tx.try_send(Box::new(operation)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => Err(()),
        }
    }
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

fn encode_cookies(records: &[CookieRecord]) -> String {
    let mut out = String::from(COOKIES_VERSION);
    out.push('\n');
    for record in records {
        push_escaped(&mut out, &record.name);
        out.push('\t');
        push_escaped(&mut out, &record.value);
        out.push('\t');
        out.push_str(&optional_millis(record.expiry));
        out.push('\t');
        push_escaped(&mut out, &record.domain);
        out.push('\t');
        push_escaped(&mut out, &record.path);
        out.push('\t');
        out.push_str(&unix_millis(record.created).to_string());
        out.push('\t');
        out.push_str(&unix_millis(record.last_access).to_string());
        out.push('\t');
        out.push(if record.host_only { '1' } else { '0' });
        out.push('\t');
        out.push(if record.secure { '1' } else { '0' });
        out.push('\t');
        out.push(if record.http_only { '1' } else { '0' });
        out.push('\t');
        out.push_str(same_site_token(record.same_site));
        out.push('\n');
    }
    out
}

fn parse_cookies(text: &str) -> io::Result<Vec<CookieRecord>> {
    let mut lines = text.lines();
    let Some(version) = lines.next() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty cookie file",
        ));
    };
    if version != COOKIES_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported cookie file",
        ));
    }
    let mut records = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        records.push(parse_cookie_line(line)?);
    }
    Ok(records)
}

fn parse_cookie_line(line: &str) -> io::Result<CookieRecord> {
    let mut parts = line.split('\t');
    let name = unescape(next_field(&mut parts, "name")?);
    let value = unescape(next_field(&mut parts, "value")?);
    let expiry = parse_optional_millis(next_field(&mut parts, "expiry")?)?;
    let domain = unescape(next_field(&mut parts, "domain")?);
    let path = unescape(next_field(&mut parts, "path")?);
    let created = millis_to_time(parse_i64(next_field(&mut parts, "created")?)?);
    let last_access = millis_to_time(parse_i64(next_field(&mut parts, "last_access")?)?);
    let host_only = parse_flag(next_field(&mut parts, "host_only")?)?;
    let secure = parse_flag(next_field(&mut parts, "secure")?)?;
    let http_only = parse_flag(next_field(&mut parts, "http_only")?)?;
    let same_site = parse_same_site(next_field(&mut parts, "same_site")?)?;
    Ok(CookieRecord {
        name,
        value,
        expiry,
        domain,
        path,
        created,
        last_access,
        host_only,
        secure,
        http_only,
        same_site,
    })
}

fn next_field<'a>(parts: &mut std::str::Split<'a, char>, name: &str) -> io::Result<&'a str> {
    parts.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("missing cookie field {name}"),
        )
    })
}

fn push_escaped(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
}

fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') | None => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

fn unix_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

fn millis_to_time(millis: i64) -> SystemTime {
    let duration = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        UNIX_EPOCH + duration
    } else {
        UNIX_EPOCH.checked_sub(duration).unwrap_or(UNIX_EPOCH)
    }
}

fn optional_millis(time: Option<SystemTime>) -> String {
    time.map(|expiry| unix_millis(expiry).to_string())
        .unwrap_or_default()
}

fn parse_optional_millis(raw: &str) -> io::Result<Option<SystemTime>> {
    if raw.is_empty() {
        return Ok(None);
    }
    Ok(Some(millis_to_time(parse_i64(raw)?)))
}

fn parse_i64(raw: &str) -> io::Result<i64> {
    raw.parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cookie timestamp"))
}

fn parse_flag(raw: &str) -> io::Result<bool> {
    match raw {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "cookie flag")),
    }
}

fn same_site_token(value: CookieSameSite) -> &'static str {
    match value {
        CookieSameSite::Strict => "Strict",
        CookieSameSite::Lax => "Lax",
        CookieSameSite::None => "None",
        CookieSameSite::Default => "Default",
    }
}

fn parse_same_site(raw: &str) -> io::Result<CookieSameSite> {
    match raw {
        "Strict" => Ok(CookieSameSite::Strict),
        "Lax" => Ok(CookieSameSite::Lax),
        "None" => Ok(CookieSameSite::None),
        "Default" => Ok(CookieSameSite::Default),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cookie same-site",
        )),
    }
}
