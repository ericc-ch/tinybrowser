//! RFC 6265bis cookie jar, above the transport.
//!
//! Storage and retrieval cite
//! [draft-ietf-httpbis-rfc6265bis](https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html)
//! section anchors, not step numbers (AGENTS.md). The backend cookie
//! feature stays off; `send()` is the only place that harvests
//! `Set-Cookie` or emits `Cookie`.

use std::fmt;
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
        self.cookies.retain(|old| !same_cookie_identity(old, &stored));
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
            parsed.max_age = avalue.parse::<i64>().ok();
        } else if aname.eq_ignore_ascii_case("Expires") {
            parsed.expires = parse_imf_fixdate(avalue);
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
        old.name == parsed.name && old.domain == domain && old.host_only == host_only && old.path == path
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
        parsed.expires.map(|expires| expires.min(now + MAX_LIFETIME))
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

fn cookie_prefixes_ok(parsed: &ParsedSetCookie, secure: bool, host_only: bool, path_attr: Option<&str>) -> bool {
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
            same_site_request
                || (context == Context::Navigation && method.is_safe())
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
    Some((url.scheme().to_owned(), registrable_domain(&canonicalize_host(url)?)))
}

fn registrable_domain(host: &str) -> String {
    if is_ip(host) {
        return host.to_owned();
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return host.to_owned();
    }
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if is_public_suffix(&last_two) {
        if labels.len() < 3 {
            return last_two;
        }
        return labels[labels.len() - 3..].join(".");
    }
    last_two
}

/// Compact PSL stand-in: no extra crate (ticket 13).
fn is_public_suffix(domain: &str) -> bool {
    let d = domain.to_ascii_lowercase();
    if !d.contains('.') {
        return d != "localhost";
    }
    matches!(
        d.as_str(),
        "co.uk"
            | "org.uk"
            | "ac.uk"
            | "gov.uk"
            | "co.jp"
            | "ne.jp"
            | "or.jp"
            | "com.au"
            | "net.au"
            | "org.au"
            | "co.nz"
            | "com.br"
            | "co.kr"
            | "com.mx"
            | "github.io"
            | "herokuapp.com"
            | "appspot.com"
    )
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

fn parse_imf_fixdate(s: &str) -> Option<SystemTime> {
    // RFC 9110 IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT`
    let s = s.trim();
    let rest = s.split_once(", ").map_or(s, |(_, r)| r);
    let mut parts = rest.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month = month_num(parts.next()?)?;
    let year: i32 = parts.next()?.parse().ok()?;
    let hms = parts.next()?;
    let mut t = hms.split(':');
    let hour: u32 = t.next()?.parse().ok()?;
    let min: u32 = t.next()?.parse().ok()?;
    let sec: u32 = t.next()?.parse().ok()?;
    let zone = parts.next()?;
    if !zone.eq_ignore_ascii_case("GMT") {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    civil_to_system(year, month, day, hour, min, sec)
}

fn month_num(m: &str) -> Option<u32> {
    Some(match m {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
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
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || min > 59 || sec > 60 {
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
