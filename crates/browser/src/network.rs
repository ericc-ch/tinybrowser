//! Browser-owned live networking: one [`net::Agent`] behind a value-only handle.
//!
//! [ADR 0010](../../../docs/adrs/0010-page-actor-ownership.md): page actors
//! receive [`FetchHandle`]. They do not expose or own [`net::Agent`].

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use net::{Agent, AgentBuilder, CookieRecord, CookieSameSite, Method};
use url::Url;

use crate::profile::{Profile, ProfileName};

/// Default per-call fetch timeout for a page-owned or browser-owned agent.
pub(crate) const PAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

const COOKIES_VERSION: &str = "tinybrowser-cookies-v1";

/// Durable backing for one Profile under `XDG_DATA_HOME`.
pub struct ProfileStore {
    name: ProfileName,
    root: Option<PathBuf>,
    disk: Mutex<DiskStore>,
}

#[derive(Clone, Copy)]
enum DiskStore {
    Memory,
    Ready,
    Corrupt,
}

impl ProfileStore {
    /// Opens the on-disk store for `profile` under the process XDG data home.
    #[must_use]
    pub fn open(profile: &Profile) -> Self {
        Self::open_in(&data_home(), profile)
    }

    /// Opens the on-disk store for `profile` under `data_home`.
    #[must_use]
    pub fn open_in(data_home: &Path, profile: &Profile) -> Self {
        Self {
            name: profile.name().clone(),
            root: Some(
                data_home
                    .join("tinybrowser")
                    .join("profiles")
                    .join(profile.name().as_str()),
            ),
            disk: Mutex::new(DiskStore::Ready),
        }
    }

    /// In-memory store: [`ProfileStore::save_from`] is a no-op.
    #[must_use]
    pub fn memory(profile: &Profile) -> Self {
        Self {
            name: profile.name().clone(),
            root: None,
            disk: Mutex::new(DiskStore::Memory),
        }
    }

    pub(crate) fn load_into(&self, agent: &Agent) {
        let Some(path) = self.cookies_path() else {
            return;
        };
        let mut disk = self.lock_disk();
        match fs::read_to_string(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                *disk = DiskStore::Ready;
            }
            Err(_) => {
                *disk = DiskStore::Corrupt;
            }
            Ok(text) => match parse_cookies(&text) {
                Ok(records) => {
                    agent.import_cookies(records);
                    *disk = DiskStore::Ready;
                }
                Err(_) => {
                    *disk = DiskStore::Corrupt;
                }
            },
        }
    }

    pub(crate) fn save_from(&self, agent: &Agent) {
        let Some(dir) = self.root.as_ref() else {
            return;
        };
        let disk = self.lock_disk();
        if matches!(*disk, DiskStore::Memory | DiskStore::Corrupt) {
            return;
        }
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let path = dir.join("cookies");
        let tmp = dir.join(format!("cookies.{}.tmp", std::process::id()));
        let encoded = encode_cookies(&agent.export_cookies());
        if fs::write(&tmp, encoded).is_ok() {
            let _ = fs::rename(tmp, path);
        } else {
            let _ = fs::remove_file(tmp);
        }
    }

    fn lock_disk(&self) -> std::sync::MutexGuard<'_, DiskStore> {
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
#[derive(Clone)]
pub struct NetworkSession {
    agent: Agent,
    store: Arc<ProfileStore>,
}

impl NetworkSession {
    /// Session from `builder`, with the default per-call fetch timeout.
    #[must_use]
    pub fn from_builder(builder: AgentBuilder, store: ProfileStore) -> Self {
        Self::from_agent(builder.timeout_per_call(PAGE_FETCH_TIMEOUT).build(), store)
    }

    /// Session that shares `agent` (and therefore its cookie jar).
    #[must_use]
    pub fn from_agent(agent: Agent, store: ProfileStore) -> Self {
        store.load_into(&agent);
        Self {
            agent,
            store: Arc::new(store),
        }
    }

    /// Value-only fetch handle for a page actor.
    #[must_use]
    pub fn fetch_handle(&self) -> FetchHandle {
        FetchHandle {
            agent: self.agent.clone(),
            store: Arc::clone(&self.store),
        }
    }

    pub(crate) fn persist(&self) {
        self.store.save_from(&self.agent);
    }

    pub(crate) fn profile_name(&self) -> ProfileName {
        self.store.profile_name().clone()
    }
}

impl Drop for NetworkSession {
    fn drop(&mut self) {
        self.persist();
    }
}

/// Cloneable, sendable handle for cookies and blocking HTTP.
///
/// Completions return to the page actor as events after `spawn_blocking`.
#[derive(Clone)]
pub struct FetchHandle {
    agent: Agent,
    store: Arc<ProfileStore>,
}

impl FetchHandle {
    pub(crate) fn from_agent(agent: Agent) -> Self {
        Self {
            agent,
            store: Arc::new(ProfileStore::memory(&Profile::default())),
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
        self.persist();
    }

    pub(crate) fn persist(&self) {
        self.store.save_from(&self.agent);
    }

    pub(crate) fn request(&self, method: Method, url: Url) -> net::RequestBuilder {
        self.agent.request(method, url)
    }
}

fn data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
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
