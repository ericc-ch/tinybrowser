//! URL decomposition for `<a>` and `<area>`
//! (<https://html.spec.whatwg.org/multipage/links.html#url-decomposition-idl-attributes>).
//!
//! The renderer links the spec-grade `url` crate, so the decomposition is a
//! thin binding: parse the element's href against the document base URL, read
//! or set one component, and serialize. Failed parses return no parts; the
//! IDL getters turn that into their per-attribute failure value, and setters
//! leave the attribute unchanged.

use rquickjs::{Ctx, Result, prelude::Func};

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
        url.query()
            .map_or_else(String::new, |query| format!("?{query}")),
        url.fragment()
            .map_or_else(String::new, |fragment| format!("#{fragment}")),
        url.origin().ascii_serialization(),
    ])
}

/// Sets one component of `spec` resolved against `base` and returns the new
/// serialization; `None` leaves the caller's attribute untouched.
fn set_part(spec: &str, base: &str, part: u32, value: &str) -> Option<String> {
    let mut url = resolve(spec, base)?;
    let changed = match part {
        PROTOCOL => url.set_scheme(value.trim_end_matches(':')).is_ok(),
        USERNAME => url.set_username(value).is_ok(),
        PASSWORD => url
            .set_password(if value.is_empty() { None } else { Some(value) })
            .is_ok(),
        HOST | HOSTNAME => url
            .set_host(if value.is_empty() { None } else { Some(value) })
            .is_ok(),
        PORT => {
            if value.is_empty() {
                url.set_port(None).is_ok()
            } else if let Ok(port) = value.parse::<u16>() {
                url.set_port(Some(port)).is_ok()
            } else {
                false
            }
        }
        PATHNAME => {
            url.set_path(value);
            true
        }
        SEARCH => {
            url.set_query(if value.is_empty() {
                None
            } else {
                Some(value.strip_prefix('?').unwrap_or(value))
            });
            true
        }
        HASH => {
            url.set_fragment(if value.is_empty() {
                None
            } else {
                Some(value.strip_prefix('#').unwrap_or(value))
            });
            true
        }
        _ => false,
    };
    changed.then(|| url.to_string())
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

/// `host` serialization: host plus a non-default port
/// (<https://url.spec.whatwg.org/#concept-url-host>).
fn host(url: &url::Url) -> String {
    let mut host = url.host_str().unwrap_or_default().to_owned();
    if let Some(port) = url.port() {
        host.push(':');
        host.push_str(&port.to_string());
    }
    host
}
