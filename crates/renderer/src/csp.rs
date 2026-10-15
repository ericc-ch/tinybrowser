//! Content Security Policy parse and enforcement.
//!
//! Policies arrive on the document response (`Content-Security-Policy` and
//! `Content-Security-Policy-Report-Only`) and on `meta http-equiv`. Fetch and
//! classic scripts consult the list before a request or an inline script runs.
//!
//! <https://w3c.github.io/webappsec-csp/#parse-serialized-policy>
//! <https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>
//! <https://w3c.github.io/webappsec-csp/#should-elements-inline-type-behavior-be-blocked-by-content-security-policy>

use base64::Engine as _;
use sha2::{Digest, Sha256, Sha384, Sha512};
use url::{Host, Origin, Url};

/// Where a policy was delivered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Header,
    Meta,
}

/// Whether a policy blocks or is only monitored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Enforce,
    Report,
}

/// One parsed policy: directive names in source order, first occurrence wins.
#[derive(Clone)]
struct Policy {
    directives: Vec<(String, Vec<String>)>,
    disposition: Disposition,
}

/// The document's CSP list and the origin `'self'` is resolved against.
#[derive(Clone, Default)]
pub(crate) struct List {
    policies: Vec<Policy>,
}

/// Inline script being checked.
pub(crate) struct InlineScript<'a> {
    /// Script source, or the `javascript:` script text.
    pub source: &'a str,
    /// Element `nonce` attribute, empty when there is none.
    pub nonce: &'a str,
    /// [Is element nonceable](https://w3c.github.io/webappsec-csp/#is-element-nonceable).
    pub nonceable: bool,
    /// Parser-inserted scripts do not get the `'strict-dynamic'` bypass.
    pub parser_inserted: bool,
    /// `javascript:` navigation uses the navigation inline type.
    pub navigation: bool,
}

/// Destination used by the pre-request check this browser enforces.
#[derive(Clone, Copy)]
pub(crate) enum Destination {
    /// `fetch` and `XMLHttpRequest` (`connect-src`).
    Connect,
    /// Classic and module script requests (`script-src-elem`).
    Script,
}

/// Metadata for a script request.
#[derive(Clone, Copy)]
pub(crate) struct ScriptRequest<'a> {
    pub nonce: &'a str,
    pub parser_inserted: bool,
}

impl List {
    /// Header policies from a navigation response. Absent headers yield an
    /// empty list, which allows every request.
    #[must_use]
    pub(crate) fn from_headers(enforce: Option<&str>, report_only: Option<&str>) -> Self {
        let mut policies = Vec::new();
        if let Some(header) = enforce {
            policies.extend(parse_policy_list(
                header,
                Source::Header,
                Disposition::Enforce,
            ));
        }
        if let Some(header) = report_only {
            policies.extend(parse_policy_list(
                header,
                Source::Header,
                Disposition::Report,
            ));
        }
        Self { policies }
    }

    /// Appends one `meta` policy. Report-only is not delivered by `meta`.
    /// `report-uri`, `frame-ancestors`, and `sandbox` are dropped
    /// (<https://w3c.github.io/webappsec-csp/#meta-element>).
    pub(crate) fn push_meta(&mut self, content: &str) {
        let mut policy = parse_serialized(content, Source::Meta, Disposition::Enforce);
        policy.directives.retain(|(name, _)| {
            !matches!(name.as_str(), "report-uri" | "frame-ancestors" | "sandbox")
        });
        if !policy.directives.is_empty() {
            self.policies.push(policy);
        }
    }

    /// Whether `url` may be fetched for `destination`.
    #[must_use]
    pub(crate) fn allows_request(
        &self,
        origin: &Origin,
        document_url: &Url,
        url: &Url,
        destination: Destination,
        script: Option<ScriptRequest<'_>>,
    ) -> bool {
        let effective = match destination {
            Destination::Connect => "connect-src",
            Destination::Script => "script-src-elem",
        };
        self.policies.iter().all(|policy| {
            policy.disposition == Disposition::Report
                || !policy_blocks_request(policy, origin, document_url, url, effective, script)
        })
    }

    /// Whether an inline classic script or `javascript:` navigation may run.
    #[must_use]
    pub(crate) fn allows_inline(&self, inline: &InlineScript<'_>) -> bool {
        self.policies.iter().all(|policy| {
            policy.disposition == Disposition::Report || !policy_blocks_inline(policy, inline)
        })
    }

    /// `fetch` and `XMLHttpRequest` pre-request check. An unparseable URL is
    /// blocked only when an enforced `connect-src` (or `default-src`) would
    /// actually run.
    #[must_use]
    pub(crate) fn allows_connect(&self, origin: &Origin, document_url: &Url, url: &str) -> bool {
        match Url::parse(url) {
            Ok(url) => self.allows_request(origin, document_url, &url, Destination::Connect, None),
            Err(_) => !self.policies.iter().any(|policy| {
                policy.disposition == Disposition::Enforce && connect_directive_executes(policy)
            }),
        }
    }
}

fn connect_directive_executes(policy: &Policy) -> bool {
    fallback_names("connect-src").iter().any(|name| {
        directive_value(policy, name).is_some() && should_execute("connect-src", name, policy)
    })
}

/// Joins every occurrence of `name` with `", "`, the way a list header is
/// extracted (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>).
#[must_use]
pub(crate) fn combined_header(headers: &[(String, String)], name: &str) -> Option<String> {
    let mut values = Vec::new();
    for (header, value) in headers {
        if header.eq_ignore_ascii_case(name) {
            let value = value.trim();
            if !value.is_empty() {
                values.push(value.to_owned());
            }
        }
    }
    if values.is_empty() {
        None
    } else {
        Some(values.join(", "))
    }
}

/// `meta http-equiv=content-security-policy` content when the element is in
/// `head` and has a `content` attribute. Other metas return `None`.
#[must_use]
pub(crate) fn meta_policy(
    base: &blitz_dom::BaseDocument,
    id: blitz_traits::node_id::NodeId,
) -> Option<String> {
    if !crate::dom::tree::is_html_element(base, id, "meta") {
        return None;
    }
    let equiv = crate::dom::tree::attr(base, id, "http-equiv")?;
    if !equiv.eq_ignore_ascii_case("content-security-policy") {
        return None;
    }
    if !ancestor_is(base, id, "head") {
        return None;
    }
    crate::dom::tree::attr(base, id, "content").map(str::to_owned)
}

fn ancestor_is(
    base: &blitz_dom::BaseDocument,
    id: blitz_traits::node_id::NodeId,
    name: &str,
) -> bool {
    let mut current = Some(id);
    while let Some(node) = current {
        if crate::dom::tree::is_html_element(base, node, name) {
            return true;
        }
        current = base.get_node(node).and_then(|found| found.parent);
    }
    false
}

/// `nonce` attribute and whether a nonce-source may match the element.
#[must_use]
pub(crate) fn element_nonce(
    base: &blitz_dom::BaseDocument,
    id: blitz_traits::node_id::NodeId,
) -> (String, bool) {
    let Some(element) = base
        .get_node(id)
        .and_then(|node| node.data.downcast_element())
    else {
        return (String::new(), false);
    };
    let mut nonce = String::new();
    let mut nonceable = false;
    for attribute in element.attrs.iter() {
        let name = attribute.name.local.as_str();
        let value = attribute.value.as_str();
        if name.eq_ignore_ascii_case("nonce") && nonce.is_empty() {
            value.clone_into(&mut nonce);
            nonceable = true;
        }
        // Dangling-markup guard
        // (<https://w3c.github.io/webappsec-csp/#is-element-nonceable>).
        let lowered_name = name.to_ascii_lowercase();
        let lowered_value = value.to_ascii_lowercase();
        for needle in ["<link", "<script", "<style"] {
            if lowered_name.contains(needle) || lowered_value.contains(needle) {
                nonceable = false;
            }
        }
    }
    if nonce.is_empty() {
        nonceable = false;
    }
    (nonce, nonceable)
}

fn parse_policy_list(header: &str, source: Source, disposition: Disposition) -> Vec<Policy> {
    split_commas(header)
        .filter_map(|token| {
            let policy = parse_serialized(token, source, disposition);
            (!policy.directives.is_empty()).then_some(policy)
        })
        .collect()
}

/// <https://w3c.github.io/webappsec-csp/#parse-serialized-policy>
fn parse_serialized(serialized: &str, _source: Source, disposition: Disposition) -> Policy {
    let mut directives = Vec::new();
    for token in serialized.split(';') {
        let token = trim_ascii(token);
        if token.is_empty() || !token.is_ascii() {
            continue;
        }
        let mut parts = split_ascii_whitespace(token);
        let Some(name) = parts.next() else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        if directives.iter().any(|(existing, _)| existing == &name) {
            continue;
        }
        let value = parts.map(str::to_owned).collect();
        directives.push((name, value));
    }
    Policy {
        directives,
        disposition,
    }
}

fn policy_blocks_request(
    policy: &Policy,
    origin: &Origin,
    document_url: &Url,
    url: &Url,
    effective: &str,
    script: Option<ScriptRequest<'_>>,
) -> bool {
    for &name in fallback_names(effective) {
        let Some(list) = directive_value(policy, name) else {
            continue;
        };
        if !should_execute(effective, name, policy) {
            continue;
        }
        let check = if name == "default-src" {
            effective
        } else {
            name
        };
        if request_check_blocks(check, list, origin, document_url, url, script) {
            return true;
        }
    }
    false
}

fn request_check_blocks(
    name: &str,
    list: &[String],
    origin: &Origin,
    document_url: &Url,
    url: &Url,
    script: Option<ScriptRequest<'_>>,
) -> bool {
    if name == "script-src-elem" || name == "script-src" {
        let script = script.unwrap_or(ScriptRequest {
            nonce: "",
            parser_inserted: true,
        });
        if nonce_matches(script.nonce, list) {
            return false;
        }
        if has_keyword(list, "'strict-dynamic'") {
            return script.parser_inserted;
        }
    }
    !url_matches_list(url, list, origin, document_url, 0)
}

fn policy_blocks_inline(policy: &Policy, inline: &InlineScript<'_>) -> bool {
    let effective = "script-src-elem";
    for &name in fallback_names(effective) {
        let Some(list) = directive_value(policy, name) else {
            continue;
        };
        if !should_execute(effective, name, policy) {
            continue;
        }
        let check = if name == "default-src" {
            effective
        } else {
            name
        };
        if (check == "script-src-elem" || check == "script-src") && !element_matches(list, inline) {
            return true;
        }
    }
    false
}

/// <https://w3c.github.io/webappsec-csp/#should-directive-execute>
fn should_execute(effective: &str, directive: &str, policy: &Policy) -> bool {
    for &fallback in fallback_names(effective) {
        if directive == fallback {
            return true;
        }
        if directive_value(policy, fallback).is_some() {
            return false;
        }
    }
    false
}

fn fallback_names(effective: &str) -> &'static [&'static str] {
    match effective {
        "script-src-elem" => &["script-src-elem", "script-src", "default-src"],
        "connect-src" => &["connect-src", "default-src"],
        _ => &[],
    }
}

fn directive_value<'a>(policy: &'a Policy, name: &str) -> Option<&'a [String]> {
    policy
        .directives
        .iter()
        .find(|(directive, _)| directive == name)
        .map(|(_, value)| value.as_slice())
}

/// <https://w3c.github.io/webappsec-csp/#match-nonce-to-source-list>
fn nonce_matches(nonce: &str, list: &[String]) -> bool {
    if nonce.is_empty() {
        return false;
    }
    list.iter().any(|expression| {
        strip_prefix_ignore_ascii_case(expression, "'nonce-")
            .and_then(|rest| rest.strip_suffix('\''))
            .is_some_and(|value| value == nonce && is_base64_value(value))
    })
}

fn has_keyword(list: &[String], keyword: &str) -> bool {
    list.iter()
        .any(|expression| expression.eq_ignore_ascii_case(keyword))
}

/// <https://w3c.github.io/webappsec-csp/#match-element-to-source-list>
fn element_matches(list: &[String], inline: &InlineScript<'_>) -> bool {
    if allows_all_inline(list) {
        return true;
    }
    let type_is_script = !inline.navigation;
    if type_is_script && inline.nonceable && nonce_matches(inline.nonce, list) {
        return true;
    }
    let unsafe_hashes = has_keyword(list, "'unsafe-hashes'");
    if !type_is_script && !unsafe_hashes {
        return false;
    }
    for expression in list {
        if expression.eq_ignore_ascii_case("'strict-dynamic'")
            && type_is_script
            && !inline.parser_inserted
        {
            return true;
        }
        if hash_matches(expression, inline.source) {
            return true;
        }
    }
    false
}

/// <https://w3c.github.io/webappsec-csp/#allow-all-inline>
fn allows_all_inline(list: &[String]) -> bool {
    let mut allow = false;
    for expression in list {
        if is_nonce_source(expression) || is_hash_source(expression) {
            return false;
        }
        if expression.eq_ignore_ascii_case("'strict-dynamic'") {
            return false;
        }
        if expression.eq_ignore_ascii_case("'unsafe-inline'") {
            allow = true;
        }
    }
    allow
}

fn hash_matches(expression: &str, source: &str) -> bool {
    let Some((algorithm, expected)) = hash_source(expression) else {
        return false;
    };
    let digest = match algorithm {
        "sha256" => {
            base64::engine::general_purpose::STANDARD.encode(Sha256::digest(source.as_bytes()))
        }
        "sha384" => {
            base64::engine::general_purpose::STANDARD.encode(Sha384::digest(source.as_bytes()))
        }
        "sha512" => {
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(source.as_bytes()))
        }
        _ => return false,
    };
    let expected = expected.replace('-', "+").replace('_', "/");
    digest == expected
}

fn is_nonce_source(expression: &str) -> bool {
    let Some(rest) = strip_prefix_ignore_ascii_case(expression, "'nonce-") else {
        return false;
    };
    let Some(value) = rest.strip_suffix('\'') else {
        return false;
    };
    is_base64_value(value)
}

fn is_hash_source(expression: &str) -> bool {
    hash_source(expression).is_some()
}

fn hash_source(expression: &str) -> Option<(&str, &str)> {
    let rest = expression.strip_prefix('\'')?;
    let (algorithm, rest) = rest.split_once('-')?;
    let algorithm = if algorithm.eq_ignore_ascii_case("sha256") {
        "sha256"
    } else if algorithm.eq_ignore_ascii_case("sha384") {
        "sha384"
    } else if algorithm.eq_ignore_ascii_case("sha512") {
        "sha512"
    } else {
        return None;
    };
    let value = rest.strip_suffix('\'')?;
    is_base64_value(value).then_some((algorithm, value))
}

fn is_base64_value(value: &str) -> bool {
    let mut saw_data = false;
    let mut padding = 0;
    for byte in value.bytes() {
        if padding > 0 {
            if byte == b'=' {
                padding += 1;
                if padding > 2 {
                    return false;
                }
                continue;
            }
            return false;
        }
        if byte == b'=' {
            if !saw_data {
                return false;
            }
            padding = 1;
            continue;
        }
        if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_') {
            saw_data = true;
        } else {
            return false;
        }
    }
    saw_data
}

/// <https://w3c.github.io/webappsec-csp/#match-url-to-source-list>
fn url_matches_list(
    url: &Url,
    list: &[String],
    origin: &Origin,
    document_url: &Url,
    redirects: u32,
) -> bool {
    if list.is_empty() {
        return false;
    }
    if list.len() == 1 && list[0].eq_ignore_ascii_case("'none'") {
        return false;
    }
    list.iter()
        .any(|expression| url_matches_expression(url, expression, origin, document_url, redirects))
}

fn url_matches_expression(
    url: &Url,
    expression: &str,
    origin: &Origin,
    document_url: &Url,
    redirects: u32,
) -> bool {
    if expression == "*" {
        return url.scheme() == "http"
            || url.scheme() == "https"
            || origin_scheme(origin)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case(url.scheme()));
    }
    if let Some(parsed) = parse_source(expression) {
        return match parsed {
            SourceExpr::Scheme(scheme) => scheme_part_matches(&scheme, url.scheme()),
            SourceExpr::Host {
                scheme,
                host,
                port,
                path,
            } => host_source_matches(
                url,
                scheme.as_deref(),
                &host,
                port.as_ref(),
                path.as_deref(),
                origin,
                redirects,
            ),
        };
    }
    if expression.eq_ignore_ascii_case("'self'") {
        return self_matches(url, origin, document_url);
    }
    false
}

enum SourceExpr {
    Scheme(String),
    Host {
        scheme: Option<String>,
        host: String,
        port: Option<PortPart>,
        path: Option<String>,
    },
}

enum PortPart {
    Star,
    Number(u16),
}

fn parse_source(expression: &str) -> Option<SourceExpr> {
    if expression.eq_ignore_ascii_case("'self'")
        || expression.starts_with('\'')
        || expression == "*"
    {
        return None;
    }
    if let Some(scheme) = expression.strip_suffix(':')
        && is_scheme(scheme)
        && !scheme.contains('/')
    {
        return Some(SourceExpr::Scheme(scheme.to_ascii_lowercase()));
    }
    let (scheme, rest) = if let Some((scheme, rest)) = expression.split_once("://") {
        if !is_scheme(scheme) {
            return None;
        }
        (Some(scheme.to_ascii_lowercase()), rest)
    } else {
        (None, expression)
    };
    if rest.is_empty() {
        return None;
    }
    let (hostport, path) = match rest.find('/') {
        Some(index) => (&rest[..index], Some(rest[index..].to_owned())),
        None => (rest, None),
    };
    if path
        .as_deref()
        .is_some_and(|path| path.contains(';') || path.contains(','))
    {
        return None;
    }
    let (host, port) = split_host_port(hostport)?;
    if !valid_host_part(host) {
        return None;
    }
    Some(SourceExpr::Host {
        scheme,
        host: host.to_ascii_lowercase(),
        port,
        path,
    })
}

fn host_source_matches(
    url: &Url,
    scheme: Option<&str>,
    host: &str,
    port: Option<&PortPart>,
    path: Option<&str>,
    origin: &Origin,
    redirects: u32,
) -> bool {
    if scheme.is_some_and(|scheme| !scheme_part_matches(scheme, url.scheme())) {
        return false;
    }
    if scheme.is_none() {
        let Some(origin_scheme) = origin_scheme(origin) else {
            return false;
        };
        if !scheme_part_matches(origin_scheme, url.scheme()) {
            return false;
        }
    }
    if !host_part_matches(host, url) {
        return false;
    }
    if !port_part_matches(port, url) {
        return false;
    }
    if redirects == 0
        && let Some(path) = path.filter(|path| !path.is_empty())
        && !path_part_matches(path, url.path())
    {
        return false;
    }
    true
}

/// <https://w3c.github.io/webappsec-csp/#match-hosts>
fn host_part_matches(pattern: &str, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if pattern == "*" {
        return true;
    }
    // Only the leading `*` is removed, so `*.example.com` becomes `.example.com`
    // (<https://w3c.github.io/webappsec-csp/#match-hosts>).
    if let Some(remaining) = pattern.strip_prefix('*')
        && remaining.starts_with('.')
    {
        return host.to_ascii_lowercase().ends_with(remaining);
    }
    pattern.eq_ignore_ascii_case(host)
}

/// <https://w3c.github.io/webappsec-csp/#match-ports>
fn port_part_matches(port: Option<&PortPart>, url: &Url) -> bool {
    let input = match port {
        None => None,
        Some(PortPart::Star) => return true,
        Some(PortPart::Number(number)) => Some(*number),
    };
    let url_port = url.port();
    if input == url_port {
        return true;
    }
    let default = default_port(url.scheme());
    if url_port.is_none() {
        return input == default;
    }
    input.is_none() && url_port == default
}

/// <https://w3c.github.io/webappsec-csp/#match-paths>
fn path_part_matches(path_a: &str, path_b: &str) -> bool {
    if path_a.is_empty() {
        return true;
    }
    if path_a == "/" && path_b.is_empty() {
        return true;
    }
    let exact = !path_a.ends_with('/');
    let mut parts_a: Vec<&str> = path_a.split('/').collect();
    let parts_b: Vec<&str> = path_b.split('/').collect();
    if parts_a.len() > parts_b.len() {
        return false;
    }
    if exact && parts_a.len() != parts_b.len() {
        return false;
    }
    if !exact {
        parts_a.pop();
    }
    for (piece_a, piece_b) in parts_a.iter().zip(parts_b.iter()) {
        let decoded_a = percent_encoding::percent_decode_str(piece_a).decode_utf8_lossy();
        let decoded_b = percent_encoding::percent_decode_str(piece_b).decode_utf8_lossy();
        if decoded_a != decoded_b {
            return false;
        }
    }
    true
}

/// <https://w3c.github.io/webappsec-csp/#match-schemes>
fn scheme_part_matches(expression_scheme: &str, url_scheme: &str) -> bool {
    expression_scheme.eq_ignore_ascii_case(url_scheme)
        || (expression_scheme.eq_ignore_ascii_case("http")
            && url_scheme.eq_ignore_ascii_case("https"))
        || (expression_scheme.eq_ignore_ascii_case("ws")
            && matches!(
                url_scheme.to_ascii_lowercase().as_str(),
                "wss" | "http" | "https"
            ))
        || (expression_scheme.eq_ignore_ascii_case("wss")
            && url_scheme.eq_ignore_ascii_case("https"))
}

fn self_matches(url: &Url, origin: &Origin, document_url: &Url) -> bool {
    if url.scheme() == "blob" {
        return false;
    }
    if same_origin(url, origin, document_url) {
        return true;
    }
    let Origin::Tuple(origin_scheme, origin_host, origin_port) = origin else {
        return false;
    };
    if !hosts_equal(origin_host, url) {
        return false;
    }
    if !self_ports(origin_scheme, *origin_port, url) {
        return false;
    }
    url.scheme() == "https"
        || url.scheme() == "wss"
        || (origin_scheme == "http" && (url.scheme() == "http" || url.scheme() == "ws"))
}

fn same_origin(url: &Url, origin: &Origin, document_url: &Url) -> bool {
    match origin {
        Origin::Tuple(..) => url.origin() == *origin,
        Origin::Opaque(_) => {
            let mut left = url.clone();
            left.set_fragment(None);
            let mut right = document_url.clone();
            right.set_fragment(None);
            left == right
        }
    }
}

fn self_ports(origin_scheme: &str, origin_port: u16, url: &Url) -> bool {
    let url_port = url.port_or_known_default();
    let same = url_port == Some(origin_port);
    let origin_default = default_port(origin_scheme) == Some(origin_port);
    let url_default = url.port().is_none();
    same || (origin_default && url_default)
}

fn hosts_equal(origin_host: &Host<String>, url: &Url) -> bool {
    match (origin_host, url.host()) {
        (Host::Domain(left), Some(Host::Domain(right))) => left.eq_ignore_ascii_case(right),
        (Host::Ipv4(left), Some(Host::Ipv4(right))) => left == &right,
        (Host::Ipv6(left), Some(Host::Ipv6(right))) => left == &right,
        _ => false,
    }
}

fn origin_scheme(origin: &Origin) -> Option<&str> {
    match origin {
        Origin::Tuple(scheme, _, _) => Some(scheme.as_str()),
        Origin::Opaque(_) => None,
    }
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        "ftp" => Some(21),
        _ => None,
    }
}

fn split_host_port(hostport: &str) -> Option<(&str, Option<PortPart>)> {
    let Some((host, port)) = hostport.rsplit_once(':') else {
        return Some((hostport, None));
    };
    if port == "*" {
        return Some((host, Some(PortPart::Star)));
    }
    if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) {
        let number = port.parse::<u16>().ok()?;
        return Some((host, Some(PortPart::Number(number))));
    }
    if hostport.contains(':') {
        return None;
    }
    Some((hostport, None))
}

fn valid_host_part(host: &str) -> bool {
    if host == "*" {
        return true;
    }
    let rest = host.strip_prefix("*.").unwrap_or(host);
    if rest.is_empty() {
        return false;
    }
    let body = rest.strip_suffix('.').unwrap_or(rest);
    if body.is_empty() {
        return false;
    }
    body.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn is_scheme(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(character) if character.is_ascii_alphabetic())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

fn split_commas(header: &str) -> impl Iterator<Item = &str> {
    header
        .split(',')
        .map(trim_ascii)
        .filter(|token| !token.is_empty())
}

fn split_ascii_whitespace(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| ['\t', '\n', '\u{000c}', '\r', ' '].contains(&character))
        .filter(|part| !part.is_empty())
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|character: char| ['\t', '\n', '\u{000c}', '\r', ' '].contains(&character))
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    if value.len() < prefix.len() {
        return None;
    }
    let (head, tail) = value.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}
