//! URL decomposition for `<a>` and `<area>`
//! (<https://html.spec.whatwg.org/multipage/links.html#url-decomposition-idl-attributes>).
//!
//! The renderer links the spec-grade `url` crate, so the decomposition is a
//! thin binding: parse the element's href against the document base URL, read
//! or set one component, and serialize. Failed parses return no parts; the
//! IDL getters turn that into their per-attribute failure value, and setters
//! leave the attribute unchanged.

use rquickjs::{Ctx, Result, prelude::Func};

use std::borrow::Cow;

use url::quirks;

/// Component indexes shared with the JavaScript accessors.
const PROTOCOL: u32 = 0;
const USERNAME: u32 = 1;
const PASSWORD: u32 = 2;
const HOST: u32 = 3;
const HOSTNAME: u32 = 4;
const PORT: u32 = 5;
const PATHNAME: u32 = 6;
const SEARCH: u32 = 7;
const HASH: u32 = 8;

/// Installs the decomposition hooks the JS shim calls.
pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    ctx.globals().set(
        "__tbUrlParts",
        Func::from(|spec: String, base: String| parts(&spec, &base)),
    )?;
    ctx.globals().set(
        "__tbUrlSetPart",
        Func::from(|spec: String, base: String, part: u32, value: String| {
            set_part(&spec, &base, part, &value)
        }),
    )?;
    Ok(())
}

/// The decomposition components of `spec` resolved against `base`, or `None`
/// when it does not parse.
fn parts(spec: &str, base: &str) -> Option<Vec<String>> {
    let url = resolve(spec, base)?;
    Some(vec![
        format!("{}:", url.scheme()),
        url.username().to_owned(),
        url.password().unwrap_or_default().to_owned(),
        host(&url),
        url.host_str().unwrap_or_default().to_owned(),
        url.port().map_or_else(String::new, |port| port.to_string()),
        url.path().to_owned(),
        // An empty query or fragment is still `null`/`""` in the
        // serialization but reads back as the empty string
        // (<https://url.spec.whatwg.org/#dom-url-search>).
        url.query()
            .filter(|query| !query.is_empty())
            .map_or_else(String::new, |query| format!("?{query}")),
        url.fragment()
            .filter(|fragment| !fragment.is_empty())
            .map_or_else(String::new, |fragment| format!("#{fragment}")),
        url.origin().ascii_serialization(),
    ])
}

/// Sets one component of `spec` resolved against `base` and returns the new
/// serialization; `None` leaves the caller's attribute untouched.
fn set_part(spec: &str, base: &str, part: u32, value: &str) -> Option<String> {
    let mut url = resolve(spec, base)?;
    // Setters run the basic URL parser on their input, which drops ASCII tab
    // and newline; `username` and `password` instead percent-encode the value
    // as-is (<https://url.spec.whatwg.org/#concept-basic-url-parser>).
    let stripped = strip_ascii_tab_newline(value);
    let changed = match part {
        PROTOCOL => quirks::set_protocol(&mut url, &stripped).is_ok(),
        USERNAME => quirks::set_username(&mut url, value).is_ok(),
        PASSWORD => quirks::set_password(&mut url, value).is_ok(),
        HOST => set_host(&mut url, &stripped, true),
        HOSTNAME => set_host(&mut url, &stripped, false),
        PORT => {
            // A value that is non-empty but loses everything to tab/newline
            // stripping leaves the port untouched; browsers do not clear it
            // (<https://url.spec.whatwg.org/#dom-url-port>).
            if !value.is_empty() && stripped.is_empty() {
                false
            } else {
                quirks::set_port(&mut url, &stripped).is_ok()
            }
        }
        PATHNAME => {
            quirks::set_pathname(&mut url, &stripped);
            true
        }
        SEARCH => {
            quirks::set_search(&mut url, &stripped);
            true
        }
        HASH => {
            quirks::set_hash(&mut url, &stripped);
            true
        }
        _ => false,
    };
    changed.then(|| url.to_string())
}

/// The browser `host`/`hostname` setters: a `file:` URL rejects a port and
/// maps a `localhost` host to the empty host
/// (<https://url.spec.whatwg.org/#concept-host-setter>).
fn set_host(url: &mut url::Url, value: &str, with_port: bool) -> bool {
    if url.scheme() == "file" && with_port && value.contains(':') {
        return false;
    }
    let changed = if with_port {
        quirks::set_host(url, value)
    } else {
        quirks::set_hostname(url, value)
    };
    if changed.is_err() {
        return false;
    }
    if url.scheme() == "file" && url.host_str() == Some("localhost") {
        return quirks::set_host(url, "").is_ok();
    }
    true
}

/// Removes ASCII tab and newline, as the basic URL parser does before every
/// setter parse (<https://url.spec.whatwg.org/#concept-basic-url-parser>).
fn strip_ascii_tab_newline(value: &str) -> Cow<'_, str> {
    if !value.contains(['\t', '\n', '\r']) {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .chars()
            .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
            .collect(),
    )
}

/// Parses `spec` relative to `base`, falling back to an absolute parse
/// (<https://url.spec.whatwg.org/#concept-basic-url-parser>). Special schemes
/// with a matching base scheme take the relative path (`http:foo.com` joins
/// the base), so base-relative parsing must come first.
fn resolve(spec: &str, base: &str) -> Option<url::Url> {
    url::Url::parse(base)
        .ok()
        .and_then(|base| base.join(spec).ok())
        .or_else(|| url::Url::parse(spec).ok())
}

/// `host` serialization: host plus a non-default port, with IPv6 literals in
/// brackets (<https://url.spec.whatwg.org/#concept-url-host>).
fn host(url: &url::Url) -> String {
    // `host_str` strips the brackets; serialization puts them back.
    let mut host = match url.host() {
        Some(url::Host::Ipv6(address)) => format!("[{address}]"),
        _ => url.host_str().unwrap_or_default().to_owned(),
    };
    if let Some(port) = url.port() {
        host.push(':');
        host.push_str(&port.to_string());
    }
    host
}
