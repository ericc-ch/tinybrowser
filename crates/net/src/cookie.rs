use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use url::{Host, Url};

use crate::initiator::InitiatorKind;
use crate::protocol::Method;

// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-cookie-lifetime-limits
const MAX_LIFETIME: Duration = Duration::from_hours(9600);
// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-storage-model
const MAX_COOKIES_PER_DOMAIN: usize = 50;
const MAX_COOKIES: usize = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SameSite {
    Strict,
    Lax,
    None,
    Default,
}

#[derive(Clone)]
struct StoredCookie {
    name: String,
    value: String,
    expiry: Option<SystemTime>,
    domain: String,
    path: String,
    created: SystemTime,
    last_access: SystemTime,
    host_only: bool,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
}

#[derive(Clone, Default)]
pub(crate) struct CookieJar {
    cookies: Vec<StoredCookie>,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieJar")
            .field("len", &self.cookies.len())
            .finish()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CookieOp<'a> {
    pub url: &'a Url,
    pub now: SystemTime,
    pub kind: RetrievalKind,
    pub initiator_kind: InitiatorKind,
    pub method: &'a Method,
    pub initiator: Option<&'a Url>,
    pub cross_site_redirect: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum RetrievalKind {
    Http,
    NonHttp,
}

impl CookieJar {
    pub(crate) fn store(&mut self, set_cookie: &str, op: CookieOp<'_>) {
        let Some(parsed) = parse_set_cookie(set_cookie) else {
            return;
        };
        let Some(stored) = receive_cookie(parsed, &op, &self.cookies) else {
            return;
        };
        self.cookies
            .retain(|old| !same_cookie_identity(old, &stored));
        self.cookies.push(stored);
        self.evict_expired(op.now);
        self.evict_excess();
    }

    pub(crate) fn cookie_string(&mut self, op: CookieOp<'_>) -> String {
        self.evict_expired(op.now);
        let Some(host) = canonicalize_host(op.url) else {
            return String::new();
        };
        let path = op.url.path();
        let mut matched: Vec<usize> = self
            .cookies
            .iter()
            .enumerate()
            .filter(|(_, cookie)| cookie.matches(&host, path, &op))
            .map(|(i, _)| i)
            .collect();
        for &i in &matched {
            self.cookies[i].last_access = op.now;
        }
        matched.sort_by(|&a, &b| {
            let left = &self.cookies[a];
            let right = &self.cookies[b];
            right
                .path
                .len()
                .cmp(&left.path.len())
                .then_with(|| left.created.cmp(&right.created))
        });
        let mut out = String::new();
        for (i, &idx) in matched.iter().enumerate() {
            let cookie = &self.cookies[idx];
            if i > 0 {
                out.push_str("; ");
            }
            if !cookie.name.is_empty() {
                out.push_str(&cookie.name);
                out.push('=');
            }
            out.push_str(&cookie.value);
        }
        out
    }

    fn evict_expired(&mut self, now: SystemTime) {
        self.cookies
            .retain(|cookie| cookie.expiry.is_none_or(|exp| exp > now));
    }

    fn evict_excess(&mut self) {
        while let Some(domain) = over_quota_domain(&self.cookies) {
            if !evict_one(&mut self.cookies, Some(&domain)) {
                break;
            }
        }
        while self.cookies.len() > MAX_COOKIES {
            if !evict_one(&mut self.cookies, None) {
                break;
            }
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<CookieRecord> {
        self.cookies
            .iter()
            .filter(|cookie| cookie.expiry.is_some())
            .map(|cookie| CookieRecord {
                name: cookie.name.clone(),
                value: cookie.value.clone(),
                expiry: cookie.expiry,
                domain: cookie.domain.clone(),
                path: cookie.path.clone(),
                created: cookie.created,
                last_access: cookie.last_access,
                host_only: cookie.host_only,
                secure: cookie.secure,
                http_only: cookie.http_only,
                same_site: CookieSameSite::from(cookie.same_site),
            })
            .collect()
    }

    pub(crate) fn restore(&mut self, records: Vec<CookieRecord>, now: SystemTime) {
        for record in records {
            if record.expiry.is_some_and(|expiry| expiry <= now) {
                continue;
            }
            let stored = StoredCookie {
                name: record.name,
                value: record.value,
                expiry: record.expiry,
                domain: record.domain,
                path: record.path,
                created: record.created,
                last_access: record.last_access,
                host_only: record.host_only,
                secure: record.secure,
                http_only: record.http_only,
                same_site: SameSite::from(record.same_site),
            };
            self.cookies
                .retain(|old| !same_cookie_identity(old, &stored));
            self.cookies.push(stored);
        }
        self.evict_expired(now);
        self.evict_excess();
    }
}

/// One persistent cookie. Session cookies (`expiry` absent) are omitted on export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieRecord {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Absolute expiry. `None` is a session cookie.
    pub expiry: Option<SystemTime>,
    /// Canonical domain.
    pub domain: String,
    /// Path prefix.
    pub path: String,
    /// Creation time.
    pub created: SystemTime,
    /// Last access time.
    pub last_access: SystemTime,
    /// Host-only flag from the storage model.
    pub host_only: bool,
    /// Secure-only flag.
    pub secure: bool,
    /// `HttpOnly` flag.
    pub http_only: bool,
    /// `SameSite` attribute.
    pub same_site: CookieSameSite,
}

/// `SameSite` value stored with a cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieSameSite {
    /// `SameSite=Strict`.
    Strict,
    /// `SameSite=Lax`.
    Lax,
    /// `SameSite=None`.
    None,
    /// No `SameSite` attribute; default Lax-like behavior in the jar.
    Default,
}

impl From<SameSite> for CookieSameSite {
    fn from(value: SameSite) -> Self {
        match value {
            SameSite::Strict => Self::Strict,
            SameSite::Lax => Self::Lax,
            SameSite::None => Self::None,
            SameSite::Default => Self::Default,
        }
    }
}

impl From<CookieSameSite> for SameSite {
    fn from(value: CookieSameSite) -> Self {
        match value {
            CookieSameSite::Strict => Self::Strict,
            CookieSameSite::Lax => Self::Lax,
            CookieSameSite::None => Self::None,
            CookieSameSite::Default => Self::Default,
        }
    }
}

fn over_quota_domain(cookies: &[StoredCookie]) -> Option<String> {
    let mut domains = HashSet::new();
    for cookie in cookies {
        domains.insert(cookie.domain.as_str());
    }
    domains.into_iter().find_map(|domain| {
        let count = cookies.iter().filter(|c| c.domain == domain).count();
        (count > MAX_COOKIES_PER_DOMAIN).then(|| domain.to_owned())
    })
}

fn evict_one(cookies: &mut Vec<StoredCookie>, domain: Option<&str>) -> bool {
    let victim = cookies
        .iter()
        .enumerate()
        .filter(|(_, cookie)| domain.is_none_or(|d| cookie.domain == d))
        .min_by(|(_, a), (_, b)| {
            let by_age = a
                .last_access
                .cmp(&b.last_access)
                .then_with(|| a.created.cmp(&b.created));
            if domain.is_none() {
                return by_age;
            }
            match (a.secure, b.secure) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => by_age,
            }
        })
        .map(|(i, _)| i);
    if let Some(i) = victim {
        cookies.remove(i);
        true
    } else {
        false
    }
}

struct ParsedSetCookie {
    name: String,
    value: String,
    max_age: Option<i64>,
    expires: Option<SystemTime>,
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    same_site: Option<SameSite>,
}

fn parse_set_cookie(input: &str) -> Option<ParsedSetCookie> {
    // https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-cookie-parsing
    if input.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F) {
        return None;
    }
    let (pair, attrs) = match input.split_once(';') {
        Some((pair, rest)) => (pair, rest),
        None => (input, ""),
    };
    let pair = pair.trim();
    let (name, value) = match pair.split_once('=') {
        Some((n, v)) => (n.trim().to_owned(), v.trim().to_owned()),
        None => (String::new(), pair.to_owned()),
    };
    if name.is_empty() && value.is_empty() {
        return None;
    }
    if name.len().saturating_add(value.len()) > 4096 {
        return None;
    }
    if has_ctl_excluding_htab(&name) || has_ctl_excluding_htab(&value) {
        return None;
    }

    let mut parsed = ParsedSetCookie {
        name,
        value,
        max_age: None,
        expires: None,
        domain: None,
        path: None,
        secure: false,
        http_only: false,
        same_site: None,
    };
    for av in attrs.split(';') {
        let av = av.trim();
        if av.is_empty() {
            continue;
        }
        let (aname, avalue) = match av.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => (av, ""),
        };
        if aname.eq_ignore_ascii_case("Max-Age") {
            if let Some(n) = parse_max_age(avalue) {
                parsed.max_age = Some(n);
            }
        } else if aname.eq_ignore_ascii_case("Expires") {
            if let Some(t) = parse_cookie_date(avalue) {
                parsed.expires = Some(t);
            }
        } else if aname.eq_ignore_ascii_case("Domain") {
            let d = avalue.strip_prefix('.').unwrap_or(avalue);
            if d.len() <= 1024 {
                parsed.domain = Some(d.to_ascii_lowercase());
            }
        } else if aname.eq_ignore_ascii_case("Path") {
            if avalue.len() <= 1024 && avalue.starts_with('/') {
                parsed.path = Some(avalue.to_owned());
            }
        } else if aname.eq_ignore_ascii_case("Secure") {
            parsed.secure = true;
        } else if aname.eq_ignore_ascii_case("HttpOnly") {
            parsed.http_only = true;
        } else if aname.eq_ignore_ascii_case("SameSite") {
            parsed.same_site = Some(if avalue.eq_ignore_ascii_case("Strict") {
                SameSite::Strict
            } else if avalue.eq_ignore_ascii_case("None") {
                SameSite::None
            } else if avalue.eq_ignore_ascii_case("Lax") {
                SameSite::Lax
            } else {
                SameSite::Default
            });
        }
    }
    Some(parsed)
}

fn parse_max_age(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn receive_cookie(
    parsed: ParsedSetCookie,
    op: &CookieOp<'_>,
    existing: &[StoredCookie],
) -> Option<StoredCookie> {
    let request_url = op.url;
    let now = op.now;
    let kind = op.kind;
    let host = canonicalize_host(request_url)?;
    let expiry = cookie_expiry(&parsed, now);
    let (host_only, domain, path, path_attr) = cookie_scope(&parsed, request_url, &host)?;
    let secure = parsed.secure;
    if secure && !is_secure_url(request_url) {
        return None;
    }
    let http_only = parsed.http_only;
    if http_only && matches!(kind, RetrievalKind::NonHttp) {
        return None;
    }
    if overlays_secure_cookie(existing, &parsed.name, &domain, &path, secure) {
        return None;
    }
    let same_site = parsed.same_site.unwrap_or(SameSite::Default);
    if same_site == SameSite::None && !secure {
        return None;
    }
    if same_site != SameSite::None
        && !op.is_same_site_request()
        && op.initiator_kind != InitiatorKind::Navigation
    {
        return None;
    }
    if !cookie_prefixes_ok(&parsed, secure, host_only, path_attr.as_deref()) {
        return None;
    }
    if let Some(old) = existing.iter().find(|old| {
        old.name == parsed.name
            && old.domain == domain
            && old.host_only == host_only
            && old.path == path
    }) {
        if matches!(kind, RetrievalKind::NonHttp) && old.http_only {
            return None;
        }
        return Some(StoredCookie {
            name: parsed.name,
            value: parsed.value,
            expiry,
            domain,
            path,
            created: old.created,
            last_access: now,
            host_only,
            secure,
            http_only,
            same_site,
        });
    }
    Some(StoredCookie {
        name: parsed.name,
        value: parsed.value,
        expiry,
        domain,
        path,
        created: now,
        last_access: now,
        host_only,
        secure,
        http_only,
        same_site,
    })
}

fn cookie_expiry(parsed: &ParsedSetCookie, now: SystemTime) -> Option<SystemTime> {
    if let Some(max_age) = parsed.max_age {
        if max_age <= 0 {
            Some(now)
        } else {
            let secs = u64::try_from(max_age).unwrap_or(u64::MAX);
            Some(now + Duration::from_secs(secs).min(MAX_LIFETIME))
        }
    } else {
        parsed
            .expires
            .map(|expires| expires.min(now + MAX_LIFETIME))
    }
}

fn cookie_scope(
    parsed: &ParsedSetCookie,
    request_url: &Url,
    host: &str,
) -> Option<(bool, String, String, Option<String>)> {
    let mut domain_attr = match parsed.domain.as_deref() {
        Some(raw) => canonicalize_domain_attr(raw)?,
        None => String::new(),
    };
    if !domain_attr.is_empty() {
        if is_ip(host) || is_public_suffix(&domain_attr) {
            if domain_attr != host {
                return None;
            }
            domain_attr.clear();
        } else if !domain_match(host, &domain_attr) {
            return None;
        }
    }
    let host_only = domain_attr.is_empty();
    let domain = if host_only {
        host.to_owned()
    } else {
        domain_attr
    };
    let path_attr = parsed.path.clone();
    let path = parsed
        .path
        .clone()
        .unwrap_or_else(|| default_path(request_url.path()));
    Some((host_only, domain, path, path_attr))
}

fn overlays_secure_cookie(
    existing: &[StoredCookie],
    name: &str,
    domain: &str,
    path: &str,
    secure: bool,
) -> bool {
    !secure
        && existing.iter().any(|old| {
            old.secure
                && old.name == name
                && (domain_match(&old.domain, domain) || domain_match(domain, &old.domain))
                && path_match(path, &old.path)
        })
}

fn cookie_prefixes_ok(
    parsed: &ParsedSetCookie,
    secure: bool,
    host_only: bool,
    path_attr: Option<&str>,
) -> bool {
    // https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-storage-model
    if parsed.name.is_empty() {
        let lvalue = parsed.value.to_ascii_lowercase();
        return !lvalue.starts_with("__secure-") && !lvalue.starts_with("__host-");
    }
    let prefix = parsed.name.to_ascii_lowercase();
    if prefix.starts_with("__secure-") && !secure {
        return false;
    }
    if prefix.starts_with("__host-") && !(secure && host_only && path_attr == Some("/")) {
        return false;
    }
    true
}

impl StoredCookie {
    fn matches(&self, host: &str, request_path: &str, op: &CookieOp<'_>) -> bool {
        let host_ok = if self.host_only {
            self.domain == host
        } else {
            domain_match(host, &self.domain)
        };
        if !host_ok {
            return false;
        }
        if !self.host_only && is_public_suffix(&self.domain) {
            return false;
        }
        if !path_match(request_path, &self.path) {
            return false;
        }
        if self.secure && !is_secure_url(op.url) {
            return false;
        }
        if self.http_only && !matches!(op.kind, RetrievalKind::Http) {
            return false;
        }
        samesite_allows(
            self.same_site,
            op.is_same_site_request(),
            op.initiator_kind,
            op.method,
        )
    }
}

fn same_cookie_identity(old: &StoredCookie, stored: &StoredCookie) -> bool {
    old.name == stored.name
        && old.domain == stored.domain
        && old.host_only == stored.host_only
        && old.path == stored.path
}

fn samesite_allows(
    same_site: SameSite,
    same_site_request: bool,
    initiator_kind: InitiatorKind,
    method: &Method,
) -> bool {
    match same_site {
        SameSite::None => true,
        SameSite::Strict => same_site_request,
        SameSite::Lax | SameSite::Default => {
            same_site_request || (initiator_kind == InitiatorKind::Navigation && method.is_safe())
        }
    }
}

impl CookieOp<'_> {
    fn is_same_site_request(&self) -> bool {
        if self.cross_site_redirect {
            return false;
        }
        match self.initiator {
            None => true,
            Some(from) => schemeful_same_site(from, self.url),
        }
    }
}

pub(crate) fn schemeful_same_site(a: &Url, b: &Url) -> bool {
    site_tuple(a) == site_tuple(b)
}

/// Site identity for renderer isolation: scheme plus registrable domain.
///
/// [ADR 0011](../../../docs/adrs/0011-renderer-processes-per-site.md). `None`
/// for opaque or non-HTTP(S) URLs; callers keep those in an opaque instance.
#[must_use]
pub fn site(url: &Url) -> Option<String> {
    let (scheme, domain) = site_tuple(url)?;
    Some(format!("{scheme}://{domain}"))
}

fn site_tuple(url: &Url) -> Option<(String, String)> {
    Some((
        url.scheme().to_owned(),
        registrable_domain(&canonicalize_host(url)?),
    ))
}

fn registrable_domain(host: &str) -> String {
    if is_ip(host) {
        return host.to_owned();
    }
    let host = host.trim_matches('.').to_ascii_lowercase();
    let suffix = public_suffix(&host);
    if host == suffix {
        return host;
    }
    let Some(prefix) = host.strip_suffix(&suffix).and_then(|p| p.strip_suffix('.')) else {
        return host;
    };
    let label = prefix.rsplit('.').next().unwrap_or(prefix);
    format!("{label}.{suffix}")
}

fn is_public_suffix(domain: &str) -> bool {
    let d = domain.trim_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return false;
    }
    public_suffix(&d) == d
}

struct SuffixList {
    rules: HashSet<Box<str>>,
    wildcards: HashSet<Box<str>>,
    exceptions: HashSet<Box<str>>,
}

fn suffix_list() -> &'static SuffixList {
    static LIST: OnceLock<SuffixList> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut rules = HashSet::new();
        let mut wildcards = HashSet::new();
        let mut exceptions = HashSet::new();
        for raw in include_str!("public_suffix_list.dat").lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if let Some(rest) = line.strip_prefix('!') {
                exceptions.insert(Box::from(rest.to_ascii_lowercase()));
            } else if let Some(rest) = line.strip_prefix("*.") {
                wildcards.insert(Box::from(rest.to_ascii_lowercase()));
            } else {
                rules.insert(Box::from(line.to_ascii_lowercase()));
            }
        }
        SuffixList {
            rules,
            wildcards,
            exceptions,
        }
    })
}

fn public_suffix(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.is_empty() {
        return String::new();
    }
    let list = suffix_list();
    let suffix_at = |i: usize| labels[i..].join(".");
    let mut matched_rule = None;
    for i in 0..labels.len() {
        let suffix = suffix_at(i);
        if list.exceptions.contains(suffix.as_str()) {
            return suffix_at(i + 1);
        }
        let parent = if i + 1 < labels.len() {
            suffix_at(i + 1)
        } else {
            String::new()
        };
        if matched_rule.is_none()
            && (list.rules.contains(suffix.as_str()) || list.wildcards.contains(parent.as_str()))
        {
            matched_rule = Some(i);
        }
    }
    if let Some(i) = matched_rule {
        return suffix_at(i);
    }
    labels[labels.len() - 1].to_owned()
}

// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#section-5.1.3
pub(crate) fn domain_match(host: &str, domain: &str) -> bool {
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    if is_ip(host) {
        return false;
    }
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();
    host.ends_with(&domain)
        && host.len() > domain.len()
        && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
}

// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#section-5.1.4
pub(crate) fn path_match(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if request_path.starts_with(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        return request_path.as_bytes().get(cookie_path.len()) == Some(&b'/');
    }
    false
}

fn default_path(uri_path: &str) -> String {
    if uri_path.is_empty() || !uri_path.starts_with('/') {
        return "/".to_owned();
    }
    if uri_path.bytes().filter(|b| *b == b'/').count() <= 1 {
        return "/".to_owned();
    }
    let end = uri_path.rfind('/').unwrap_or(0);
    uri_path[..end].to_owned()
}

fn canonicalize_host(url: &Url) -> Option<String> {
    match url.host()? {
        Host::Domain(d) => Some(d.to_ascii_lowercase()),
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(ip.to_string()),
    }
}

fn canonicalize_domain_attr(raw: &str) -> Option<String> {
    let stripped = raw.strip_prefix('.').unwrap_or(raw);
    if stripped.len() > 1024 {
        return None;
    }
    match Host::parse(stripped).ok()? {
        Host::Domain(d) => Some(d),
        Host::Ipv4(ip) => Some(ip.to_string()),
        Host::Ipv6(ip) => Some(ip.to_string()),
    }
}

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

fn is_secure_url(url: &Url) -> bool {
    // https://html.spec.whatwg.org/multipage/webappapis.html#secure-contexts
    if url.scheme() == "https" || url.scheme() == "wss" {
        return true;
    }
    if url.scheme() != "http" && url.scheme() != "ws" {
        return false;
    }
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        None => false,
    }
}

fn has_ctl_excluding_htab(s: &str) -> bool {
    s.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F)
}

// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-dates
fn parse_cookie_date(s: &str) -> Option<SystemTime> {
    let mut hour = None;
    let mut min = None;
    let mut sec = None;
    let mut day = None;
    let mut month = None;
    let mut year = None;

    for token in cookie_date_tokens(s) {
        if hour.is_none()
            && let Some((h, m, s)) = parse_time_token(token)
        {
            hour = Some(h);
            min = Some(m);
            sec = Some(s);
            continue;
        }
        if day.is_none()
            && let Some(d) = parse_day_token(token)
        {
            day = Some(d);
            continue;
        }
        if month.is_none()
            && let Some(m) = month_num(token)
        {
            month = Some(m);
            continue;
        }
        if year.is_none()
            && let Some(y) = parse_year_token(token)
        {
            year = Some(y);
        }
    }

    let year = year?;
    let month = month?;
    let day = day?;
    let hour = hour?;
    let min = min?;
    let sec = sec?;
    if year < 1601 || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    if day > days_in_month(year, month)? {
        return None;
    }
    civil_to_system(year, month, day, hour, min, sec)
}

fn is_cookie_date_delimiter(b: u8) -> bool {
    matches!(b, 0x09 | 0x20..=0x2F | 0x3B..=0x40 | 0x5B..=0x60 | 0x7B..=0x7E)
}

fn cookie_date_tokens(s: &str) -> impl Iterator<Item = &str> {
    s.as_bytes()
        .split(|b| is_cookie_date_delimiter(*b))
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
}

fn parse_day_token(token: &str) -> Option<u32> {
    let (n, rest) = take_ascii_digits(token.as_bytes(), 1, 2)?;
    if rest.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    u32::try_from(n).ok()
}

fn parse_time_token(token: &str) -> Option<(u32, u32, u32)> {
    let bytes = token.as_bytes();
    let (hour, rest) = take_ascii_digits(bytes, 1, 2)?;
    let rest = rest.strip_prefix(b":")?;
    let (min, rest) = take_ascii_digits(rest, 1, 2)?;
    let rest = rest.strip_prefix(b":")?;
    let (sec, rest) = take_ascii_digits(rest, 1, 2)?;
    if rest.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some((
        u32::try_from(hour).ok()?,
        u32::try_from(min).ok()?,
        u32::try_from(sec).ok()?,
    ))
}

fn parse_year_token(token: &str) -> Option<i32> {
    let (n, rest) = take_ascii_digits(token.as_bytes(), 2, 4)?;
    if rest.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some(if (70..=99).contains(&n) {
        n + 1900
    } else if (0..=69).contains(&n) {
        n + 2000
    } else {
        n
    })
}

fn take_ascii_digits(bytes: &[u8], min: usize, max: usize) -> Option<(i32, &[u8])> {
    let mut n = 0usize;
    while n < bytes.len() && n < max && bytes[n].is_ascii_digit() {
        n += 1;
    }
    if n < min {
        return None;
    }
    let value = std::str::from_utf8(&bytes[..n]).ok()?.parse().ok()?;
    Some((value, &bytes[n..]))
}

fn month_num(m: &str) -> Option<u32> {
    let b = m.as_bytes();
    if b.len() < 3 {
        return None;
    }
    let key = [
        b[0].to_ascii_lowercase(),
        b[1].to_ascii_lowercase(),
        b[2].to_ascii_lowercase(),
    ];
    Some(match &key {
        b"jan" => 1,
        b"feb" => 2,
        b"mar" => 3,
        b"apr" => 4,
        b"may" => 5,
        b"jun" => 6,
        b"jul" => 7,
        b"aug" => 8,
        b"sep" => 9,
        b"oct" => 10,
        b"nov" => 11,
        b"dec" => 12,
        _ => return None,
    })
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    Some(match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => return None,
    })
}

fn civil_to_system(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
) -> Option<SystemTime> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = y.div_euclid(400);
    let yoe = u64::try_from(y.rem_euclid(400)).ok()?;
    let mp = u64::from(if month > 2 { month - 3 } else { month + 9 });
    let doy = (153 * mp + 2) / 5 + u64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era
        .checked_mul(146_097)?
        .checked_add(i64::try_from(doe).ok()?)?
        - 719_468;
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec))?;
    if secs >= 0 {
        Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).ok()?))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(u64::try_from(-secs).ok()?))
    }
}
