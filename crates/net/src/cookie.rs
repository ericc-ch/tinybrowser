//! RFC 6265bis cookie jar, above the transport.
//!
//! Storage and retrieval cite
//! [draft-ietf-httpbis-rfc6265bis](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html)
//! section anchors, not step numbers (AGENTS.md). The backend cookie
//! feature stays off; `send()` is the only place that harvests
//! `Set-Cookie` or emits `Cookie`.

use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use url::{Host, Url};

use crate::context::Context;
use crate::method::Method;

/// 400-day lifetime cap
/// ([§5.5](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-cookie-lifetime-limits)).
const MAX_LIFETIME: Duration = Duration::from_hours(9600);

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
    host_only: bool,
    secure: bool,
    http_only: bool,
    same_site: SameSite,
}

/// In-memory cookie store. `Agent` wraps this in `Arc<Mutex<_>>`.
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

/// Retrieval/storage inputs shared by HTTP `send()`, `document.cookie`, and
/// the WebSocket handshake.
#[derive(Clone, Copy)]
pub(crate) struct CookieOp<'a> {
    pub url: &'a Url,
    pub now: SystemTime,
    pub kind: RetrievalKind,
    pub context: Context,
    pub method: &'a Method,
    pub initiator: Option<&'a Url>,
}

#[derive(Clone, Copy)]
pub(crate) enum RetrievalKind {
    /// `Cookie` request header.
    Http,
    /// `document.cookie` / [`crate::Agent::cookies_for`].
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
    }

    pub(crate) fn cookie_string(&mut self, op: CookieOp<'_>) -> String {
        self.evict_expired(op.now);
        let Some(host) = canonicalize_host(op.url) else {
            return String::new();
        };
        let path = op.url.path();

        let mut matched: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|cookie| cookie.matches(&host, path, &op))
            .collect();
        matched.sort_by(|a, b| {
            b.path
                .len()
                .cmp(&a.path.len())
                .then_with(|| a.created.cmp(&b.created))
        });
        let mut out = String::new();
        for (i, cookie) in matched.iter().enumerate() {
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
    // §5.6: CTL excluding HTAB aborts the whole string.
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
            // Ignore this cookie-av on parse failure; do not clear a prior
            // Max-Age in the same header (RFC 6265bis storage model).
            if let Ok(n) = avalue.parse::<i64>() {
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
        && !is_same_site_request(op.initiator, request_url)
        && op.context != Context::Navigation
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
    let domain_attr = parsed.domain.clone().unwrap_or_default();
    if !domain_attr.is_empty() {
        if domain_attr.bytes().any(|b| b > 127) {
            return None;
        }
        if is_public_suffix(&domain_attr) {
            return None;
        }
        if !domain_match(host, &domain_attr) {
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
    let lname = parsed.name.to_ascii_lowercase();
    let lvalue = parsed.value.to_ascii_lowercase();
    let prefix = if lname.is_empty() {
        lvalue.as_str()
    } else {
        lname.as_str()
    };
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
            is_same_site_request(op.initiator, op.url),
            op.context,
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
    context: Context,
    method: &Method,
) -> bool {
    match same_site {
        SameSite::None => true,
        SameSite::Strict => same_site_request,
        SameSite::Lax | SameSite::Default => {
            same_site_request || (context == Context::Navigation && method.is_safe())
        }
    }
}

fn is_same_site_request(initiator: Option<&Url>, target: &Url) -> bool {
    match initiator {
        None => true,
        Some(from) => schemeful_same_site(from, target),
    }
}

/// Scheme plus registrable domain (PSL stand-in).
fn schemeful_same_site(a: &Url, b: &Url) -> bool {
    site_tuple(a) == site_tuple(b)
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

/// Vendored PSL (<https://publicsuffix.org/list/public_suffix_list.dat>), no crate.
/// Matching: exception, then longest rule, then implicit `*`.
/// `localhost` is not a suffix so host-only cookies still store.
fn is_public_suffix(domain: &str) -> bool {
    let d = domain.trim_matches('.').to_ascii_lowercase();
    if d == "localhost" || d.is_empty() {
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

/// [§5.1.3](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#section-5.1.3)
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

/// [§5.1.4](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#section-5.1.4)
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

fn is_ip(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
}

fn is_secure_url(url: &Url) -> bool {
    url.scheme() == "https" || url.scheme() == "wss"
}

fn has_ctl_excluding_htab(s: &str) -> bool {
    s.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7F)
}

/// Cookie-date parse
/// ([RFC 6265bis §5.1.1](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html#name-dates)).
///
/// Splits on the spec delimiter octet set, then picks time / day / month /
/// year from each token. Year-value 70–99 adds 1900, 0–69 adds 2000; year
/// below 1601 and dates that do not exist abort.
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

/// `delimiter` in RFC 6265bis §5.1.1 (`%x09 / %x20-2F / %x3B-40 / %x5B-60 / %x7B-7E`).
fn is_cookie_date_delimiter(b: u8) -> bool {
    matches!(b, 0x09 | 0x20..=0x2F | 0x3B..=0x40 | 0x5B..=0x60 | 0x7B..=0x7E)
}

fn cookie_date_tokens(s: &str) -> impl Iterator<Item = &str> {
    s.as_bytes()
        .split(|b| is_cookie_date_delimiter(*b))
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
}

/// `1*2DIGIT [ non-digit *OCTET ]`. A leftover leading digit means the
/// token is not a day (so `1994` is a year, not day 19).
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

/// `2*4DIGIT [ non-digit *OCTET ]`, then the 0–69 / 70–99 century rule
/// on the numeric year-value.
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
    // First three octets, ASCII-folded. Byte indexing avoids panicking on
    // a mid-codepoint slice of a hostile Expires month token.
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

#[cfg(test)]
mod public_suffix_lookup {
    use super::{is_public_suffix, registrable_domain};

    #[test]
    fn s3_amazonaws_com_is_the_suffix_not_amazonaws_com() {
        assert!(is_public_suffix("s3.amazonaws.com"));
        assert!(!is_public_suffix("evil.s3.amazonaws.com"));
        assert_eq!(
            registrable_domain("evil.s3.amazonaws.com"),
            "evil.s3.amazonaws.com"
        );
        assert_eq!(registrable_domain("www.example.com"), "example.com");
        assert_eq!(registrable_domain("www.bbc.co.uk"), "bbc.co.uk");
    }

    #[test]
    fn wildcard_and_exception_rules_apply() {
        // PSL: `*.ck` with exception `!www.ck`.
        assert!(is_public_suffix("foo.ck"));
        assert!(!is_public_suffix("www.ck"));
        assert_eq!(registrable_domain("bar.foo.ck"), "bar.foo.ck");
        assert_eq!(registrable_domain("www.ck"), "www.ck");
    }
}

#[cfg(test)]
mod same_site_none_retrieval {
    use super::{SameSite, samesite_allows};
    use crate::context::Context;
    use crate::method::Method;

    #[test]
    fn none_is_sent_on_cross_site_fetch() {
        assert!(samesite_allows(
            SameSite::None,
            false,
            Context::Fetch,
            &Method::GET
        ));
    }
}

#[cfg(test)]
mod cookie_date_parse {
    use super::parse_cookie_date;
    use std::time::SystemTime;

    #[test]
    fn hyphenated_two_digit_year_and_imf_form() {
        let past = parse_cookie_date("Wed, 09-Jun-01 10:18:14 GMT").expect("past");
        assert!(past < SystemTime::now());
        let future = parse_cookie_date("Sun, 06 Nov 2094 08:49:37 GMT").expect("future");
        assert!(future > SystemTime::now());
    }

    #[test]
    fn slash_delimited_netscape_form_parses() {
        let past = parse_cookie_date("09/Nov/1999 23:12:40 GMT").expect("slash date");
        assert!(past < SystemTime::now());
    }

    #[test]
    fn year_below_1601_and_impossible_day_fail() {
        assert!(parse_cookie_date("Sun, 06 Nov 1600 08:49:37 GMT").is_none());
        assert!(parse_cookie_date("Sun, 31 Feb 2094 08:49:37 GMT").is_none());
    }

    #[test]
    fn three_digit_year_gets_the_century_rule() {
        // 099 → 99 → 1999 (70–99 add 1900), not rejected for length 3.
        let past = parse_cookie_date("Sun, 06 Nov 099 08:49:37 GMT").expect("3-digit year");
        assert!(past < SystemTime::now());
    }

    #[test]
    fn non_ascii_month_token_does_not_panic() {
        // Before the fix, month_num sliced m[..3] and panicked on this token.
        assert!(parse_cookie_date("Wed, 09 ááá 2094 08:49:37 GMT").is_none());
    }
}
