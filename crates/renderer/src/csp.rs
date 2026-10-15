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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha384, Sha512};
use url::{Host, Origin, Url};

/// Where a policy was delivered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Header,
    Meta,
}

/// Whether a policy blocks or is only monitored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Disposition {
    Enforce,
    Report,
}

/// One parsed policy: directive names in source order, first occurrence wins.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Policy {
    directives: Vec<(String, Vec<String>)>,
    disposition: Disposition,
    /// The serialized policy as delivered, reported as `originalPolicy`
    /// (<https://w3c.github.io/webappsec-csp/#parse-serialized-policy>).
    serialized: String,
}

/// A decision to block (or, for report-only policies, to report) one check.
/// Callers dispatch one `securitypolicyviolation` event per violation
/// (<https://w3c.github.io/webappsec-csp/#report-violation>).
pub(crate) struct Violation {
    /// The directive that blocked, after fallback: `script-src-elem` from a
    /// `script-src` policy reports `script-src`
    /// (<https://w3c.github.io/webappsec-csp/#violation-effective-directive>).
    pub effective_directive: String,
    pub original_policy: String,
    pub disposition: Disposition,
    /// The blocked URL, or `inline` / `eval` for non-fetch checks.
    pub blocked_uri: String,
    /// First 40 characters of an inline script or style, when applicable.
    pub sample: Option<String>,
}

impl Violation {
    /// Whether this violation blocks; report-only policies only report
    /// (<https://w3c.github.io/webappsec-csp/#disposition>).
    #[must_use]
    pub(crate) fn blocks(&self) -> bool {
        self.disposition == Disposition::Enforce
    }
}

/// True when any violation comes from an enforced policy.
#[must_use]
pub(crate) fn blocks(violations: &[Violation]) -> bool {
    violations.iter().any(Violation::blocks)
}

/// The document's CSP list and the origin `'self'` is resolved against.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct List {
    policies: Vec<Policy>,
}

/// Inline content being checked: an inline script, `javascript:` navigation,
/// event handler, style block, or style attribute.
pub(crate) struct InlineScript<'a> {
    /// Script or style source.
    pub source: &'a str,
    /// Element `nonce` attribute, empty when there is none.
    pub nonce: &'a str,
    /// [Is element nonceable](https://w3c.github.io/webappsec-csp/#is-element-nonceable).
    pub nonceable: bool,
    /// Parser-inserted scripts do not get the `'strict-dynamic'` bypass.
    pub parser_inserted: bool,
    /// What kind of inline behavior is being checked.
    pub kind: InlineKind,
}

/// The behavior an inline check governs.
#[derive(Clone, Copy)]
pub(crate) enum InlineKind {
    /// Inline classic script (`script-src-elem`).
    Script,
    /// `javascript:` navigation (`script-src-elem`).
    Navigation,
    /// Event handler content attribute (`script-src-attr`).
    ScriptAttribute,
    /// `eval()` / `Function()` (`script-src`).
    Evaluation,
}

/// Destination used by the pre-request check this browser enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Destination {
    /// `fetch` and `XMLHttpRequest` (`connect-src`).
    Connect,
    /// Classic and module script requests (`script-src-elem`).
    Script,
    /// Stylesheet requests: `<link rel=stylesheet>` and `@import`
    /// (`style-src-elem`).
    StyleSheet,
    /// Images (`img-src`).
    Img,
    /// Web fonts (`font-src`).
    Font,
}

/// The directive that decides a check
/// (<https://w3c.github.io/webappsec-csp/#effective-directive>).
#[derive(Clone, Copy)]
enum Effective {
    ScriptSrcElem,
    ScriptSrcAttr,
    /// String compilation (`eval`, `Function`).
    ScriptSrc,
    StyleSrcElem,
    ConnectSrc,
    ImgSrc,
    FontSrc,
}

impl Effective {
    fn name(self) -> &'static str {
        match self {
            Self::ScriptSrcElem => "script-src-elem",
            Self::ScriptSrcAttr => "script-src-attr",
            Self::ScriptSrc => "script-src",
            Self::StyleSrcElem => "style-src-elem",
            Self::ConnectSrc => "connect-src",
            Self::ImgSrc => "img-src",
            Self::FontSrc => "font-src",
        }
    }

    /// The fallback list from most to least relevant, the directive itself
    /// included (<https://w3c.github.io/webappsec-csp/#directive-fallback-list>).
    fn fallback(self) -> &'static [&'static str] {
        match self {
            Self::ScriptSrcElem => &["script-src-elem", "script-src", "default-src"],
            Self::ScriptSrcAttr => &["script-src-attr", "script-src", "default-src"],
            Self::ScriptSrc => &["script-src", "default-src"],
            Self::StyleSrcElem => &["style-src-elem", "style-src", "default-src"],
            Self::ConnectSrc => &["connect-src", "default-src"],
            Self::ImgSrc => &["img-src", "default-src"],
            Self::FontSrc => &["font-src", "default-src"],
        }
    }
}

impl Destination {
    /// The effective directive for this destination.
    fn effective(self) -> Effective {
        match self {
            Self::Connect => Effective::ConnectSrc,
            Self::Script => Effective::ScriptSrcElem,
            Self::StyleSheet => Effective::StyleSrcElem,
            Self::Img => Effective::ImgSrc,
            Self::Font => Effective::FontSrc,
        }
    }
}

impl InlineScript<'_> {
    /// The effective directive for this inline check.
    fn effective(&self) -> Effective {
        match self.kind {
            InlineKind::Script | InlineKind::Navigation => Effective::ScriptSrcElem,
            InlineKind::ScriptAttribute => Effective::ScriptSrcAttr,
            InlineKind::Evaluation => Effective::ScriptSrc,
        }
    }

    /// `blockedURI` for an inline check: `inline`, or `eval` for evaluation
    /// (<https://w3c.github.io/webappsec-csp/#violation-blocked-uri>).
    fn blocked_uri(&self) -> String {
        match self.kind {
            InlineKind::Evaluation => "eval".to_owned(),
            InlineKind::Script | InlineKind::Navigation | InlineKind::ScriptAttribute => {
                "inline".to_owned()
            }
        }
    }

    /// First 40 characters of the source
    /// (<https://w3c.github.io/webappsec-csp/#violation-sample>).
    fn sample(&self) -> String {
        self.source.chars().take(40).collect()
    }
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

    /// The violations `url` produces for `destination`, one per policy. An
    /// empty result allows the request; report-only policies appear without
    /// blocking
    /// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
    #[must_use]
    pub(crate) fn check_request(
        &self,
        origin: &Origin,
        document_url: &Url,
        url: &Url,
        destination: Destination,
        script: Option<ScriptRequest<'_>>,
    ) -> Vec<Violation> {
        self.violations(origin, document_url, url, destination, script, 0)
    }

    /// The violations a response URL produces after redirects: the
    /// post-request check re-runs the source lists with paths ignored
    /// (<https://w3c.github.io/webappsec-csp/#should-block-response>).
    #[must_use]
    pub(crate) fn check_redirected_request(
        &self,
        origin: &Origin,
        document_url: &Url,
        url: &Url,
        destination: Destination,
    ) -> Vec<Violation> {
        self.violations(origin, document_url, url, destination, None, 1)
    }

    fn violations(
        &self,
        origin: &Origin,
        document_url: &Url,
        url: &Url,
        destination: Destination,
        script: Option<ScriptRequest<'_>>,
        redirects: u32,
    ) -> Vec<Violation> {
        let effective = destination.effective();
        let mut violations = Vec::new();
        for policy in &self.policies {
            if policy_request_violation(
                policy,
                origin,
                document_url,
                url,
                destination,
                script,
                redirects,
            ) {
                violations.push(Violation {
                    effective_directive: effective.name().to_owned(),
                    original_policy: policy.serialized.clone(),
                    disposition: policy.disposition,
                    blocked_uri: url.as_str().to_owned(),
                    sample: None,
                });
            }
        }
        violations
    }

    /// The violations an inline classic script or `javascript:` navigation
    /// produces
    /// (<https://w3c.github.io/webappsec-csp/#should-elements-inline-type-behavior-be-blocked-by-content-security-policy>).
    #[must_use]
    pub(crate) fn check_inline(&self, inline: &InlineScript<'_>) -> Vec<Violation> {
        let effective = inline.effective();
        let mut violations = Vec::new();
        for policy in &self.policies {
            if policy_inline_violation(policy, inline) {
                violations.push(Violation {
                    effective_directive: effective.name().to_owned(),
                    original_policy: policy.serialized.clone(),
                    disposition: policy.disposition,
                    blocked_uri: inline.blocked_uri(),
                    sample: Some(inline.sample()),
                });
            }
        }
        violations
    }
}

/// Tuple origin carried with a dial. An opaque document origin is `None`,
/// and `'self'` then matches only the document URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TupleOrigin {
    scheme: String,
    host: String,
    port: u16,
}

impl TupleOrigin {
    fn from_origin(origin: &Origin) -> Option<Self> {
        match origin {
            Origin::Tuple(scheme, host, port) => Some(Self {
                scheme: scheme.clone(),
                host: host.to_string(),
                port: *port,
            }),
            Origin::Opaque(_) => None,
        }
    }

    fn to_origin(&self) -> Origin {
        let host = Host::parse(&self.host).unwrap_or(Host::Domain(self.host.clone()));
        Origin::Tuple(self.scheme.clone(), host, self.port)
    }
}

/// The policy a subresource dial carries so every redirect hop is checked
/// before it is followed. `None` on a dial skips the check, which is how a
/// document navigation is sent.
///
/// <https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RedirectGate {
    list: List,
    origin: Option<TupleOrigin>,
    destination: Destination,
}

impl RedirectGate {
    pub(crate) fn new(list: List, origin: &Origin, destination: Destination) -> Self {
        Self {
            list,
            origin: TupleOrigin::from_origin(origin),
            destination,
        }
    }

    /// Whether `url` may be fetched. `redirects` is `0` for the original URL
    /// and at least `1` after a redirect, which skips path matching.
    /// Report-only policies do not refuse the hop.
    #[must_use]
    pub fn allows(&self, document_url: &Url, url: &Url, redirects: u32) -> bool {
        let origin = self
            .origin
            .as_ref()
            .map_or_else(Origin::new_opaque, TupleOrigin::to_origin);
        !blocks(&self.list.violations(
            &origin,
            document_url,
            url,
            self.destination,
            None,
            redirects,
        ))
    }
}

/// Whether the fallback directive chain for `destination` violates `policy`
/// (<https://w3c.github.io/webappsec-csp/#should-request-be-blocked-by-content-security-policy>).
fn policy_request_violation(
    policy: &Policy,
    origin: &Origin,
    document_url: &Url,
    url: &Url,
    destination: Destination,
    script: Option<ScriptRequest<'_>>,
    redirects: u32,
) -> bool {
    let effective = destination.effective();
    for &name in effective.fallback() {
        let Some(list) = directive_value(policy, name) else {
            continue;
        };
        // A directive lower in the fallback chain only executes when no
        // higher-priority directive is present; the loop order guarantees
        // that, so the first present directive decides.
        return request_check_blocks(
            effective,
            list,
            origin,
            document_url,
            url,
            script,
            redirects,
        );
    }
    false
}

/// Whether the fallback chain for the inline kind violates `policy`.
fn policy_inline_violation(policy: &Policy, inline: &InlineScript<'_>) -> bool {
    let effective = inline.effective();
    for &name in effective.fallback() {
        let Some(list) = directive_value(policy, name) else {
            continue;
        };
        return inline_check_blocks(effective, list, inline);
    }
    false
}

/// Joins every occurrence of `name` with `", "`, the way a list header is
/// extracted (<https://fetch.spec.whatwg.org/#concept-header-list-get-decode-split>).
#[must_use]
pub(crate) fn combined_header(headers: &[(String, String)], name: &str) -> Option<String> {
    net::combined_header(
        headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str()),
    )
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
    slot: Option<&str>,
) -> (String, bool) {
    let Some(element) = base
        .get_node(id)
        .and_then(|node| node.data.downcast_element())
    else {
        return (String::new(), false);
    };
    // `clear_nonce_on_connect` blanks the content attribute and moves the
    // value to the internal slot; checks use the slot when it exists
    // (<https://html.spec.whatwg.org/multipage/urls-and-fetching.html#nonce-attributes>).
    let mut nonce = slot.unwrap_or_default().to_owned();
    let mut nonceable = !nonce.is_empty();
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
        serialized: trim_ascii(serialized).to_owned(),
    }
}

fn request_check_blocks(
    effective: Effective,
    list: &[String],
    origin: &Origin,
    document_url: &Url,
    url: &Url,
    script: Option<ScriptRequest<'_>>,
    redirects: u32,
) -> bool {
    // The pre-request check short-circuits on nonces and `'strict-dynamic'`.
    // The post-request (redirect) check is URL-only, so a policy with
    // `'strict-dynamic'` does not block every response
    // (<https://w3c.github.io/webappsec-csp/#script-pre-request>).
    if redirects == 0 && matches!(effective, Effective::ScriptSrcElem | Effective::ScriptSrc) {
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
    !url_matches_list(url, list, origin, document_url, redirects)
}

/// The inline check for one directive
/// (<https://w3c.github.io/webappsec-csp/#should-block-inline>).
fn inline_check_blocks(effective: Effective, list: &[String], inline: &InlineScript<'_>) -> bool {
    match effective {
        Effective::ScriptSrc => evaluation_blocks(list, inline.source),
        Effective::ScriptSrcElem | Effective::ScriptSrcAttr => !element_matches(list, inline),
        Effective::StyleSrcElem
        | Effective::ConnectSrc
        | Effective::ImgSrc
        | Effective::FontSrc => false,
    }
}

/// Whether a source list blocks compiling `source`
/// (<https://w3c.github.io/webappsec-csp/#can-compile-strings>): only
/// `'unsafe-eval'` and matching hash-sources allow evaluation.
fn evaluation_blocks(list: &[String], source: &str) -> bool {
    if has_keyword(list, "'unsafe-eval'") {
        return false;
    }
    !list
        .iter()
        .any(|expression| hash_matches(expression, source))
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
    if allows_all_inline(list, inline.kind) {
        return true;
    }
    // Nonces apply to inline script and style elements, not to attributes of
    // either element or to `javascript:` navigations
    // (<https://w3c.github.io/webappsec-csp/#match-element-to-source-list>).
    let nonce_applies = matches!(inline.kind, InlineKind::Script);
    if nonce_applies && inline.nonceable && nonce_matches(inline.nonce, list) {
        return true;
    }
    let unsafe_hashes = has_keyword(list, "'unsafe-hashes'");
    let hash_applies = matches!(inline.kind, InlineKind::Script) || unsafe_hashes;
    if !hash_applies {
        return false;
    }
    for expression in list {
        if expression.eq_ignore_ascii_case("'strict-dynamic'")
            && matches!(inline.kind, InlineKind::Script)
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
fn allows_all_inline(list: &[String], kind: InlineKind) -> bool {
    let mut allow = false;
    for expression in list {
        if is_nonce_source(expression) || is_hash_source(expression) {
            return false;
        }
        if matches!(
            kind,
            InlineKind::Script | InlineKind::ScriptAttribute | InlineKind::Navigation
        ) && expression.eq_ignore_ascii_case("'strict-dynamic'")
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn url(spec: &str) -> Url {
        Url::parse(spec).expect("test URL")
    }

    fn inline(source: &str, parser_inserted: bool) -> InlineScript<'_> {
        InlineScript {
            source,
            nonce: "",
            nonceable: false,
            parser_inserted,
            kind: InlineKind::Script,
        }
    }

    #[test]
    fn directive_names_lowercase_and_first_occurrence_wins() {
        let list = List::from_headers(Some("script-SRC 'self'; SCRIPT-SRC 'none'"), None);
        let origin = url("https://example.test/").origin();
        let document_url = url("https://example.test/page");
        // The duplicate `'none'` must not override the first `'self'`.
        assert!(
            list.check_request(
                &origin,
                &document_url,
                &url("https://example.test/a.js"),
                Destination::Script,
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn non_ascii_directives_are_dropped() {
        let list = List::from_headers(Some("script-src \u{e9}"), None);
        let origin = url("https://example.test/").origin();
        let document_url = url("https://example.test/page");
        // The only token is not ASCII, so the policy parses empty and allows.
        assert!(
            list.check_request(
                &origin,
                &document_url,
                &url("https://other.test/a.js"),
                Destination::Script,
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn wildcard_host_matches_subdomains_only() {
        assert!(host_part_matches(
            "*.example.test",
            &url("https://a.example.test/")
        ));
        assert!(!host_part_matches(
            "*.example.test",
            &url("https://example.test/")
        ));
    }

    #[test]
    fn ports_compare_with_scheme_defaults() {
        assert!(port_part_matches(
            Some(&PortPart::Star),
            &url("https://example.test:8443/")
        ));
        assert!(port_part_matches(
            Some(&PortPart::Number(443)),
            &url("https://example.test/")
        ));
        assert!(!port_part_matches(
            Some(&PortPart::Number(443)),
            &url("http://example.test/")
        ));
    }

    #[test]
    fn paths_match_exactly_or_by_directory() {
        assert!(path_part_matches("/a/b", "/a/b"));
        assert!(!path_part_matches("/a/b", "/a/c"));
        assert!(path_part_matches("/a/", "/a/b"));
        assert!(!path_part_matches("/a", "/a/b"));
        assert!(!path_part_matches("/a/b/", "/a/b"));
    }

    #[test]
    fn split_host_port_table() {
        let cases = [
            ("example.test", Some(("example.test", None))),
            ("example.test:443", Some(("example.test", Some(443u16)))),
            ("example.test:", None),
            ("example.test:abc", None),
            ("example.test:99999", None),
        ];
        for (input, expect) in cases {
            let parsed = split_host_port(input);
            match expect {
                None => assert!(parsed.is_none(), "{input}"),
                Some((host, None)) => {
                    let (got, port) = parsed.expect(input);
                    assert_eq!(got, host, "{input}");
                    assert!(port.is_none(), "{input}");
                }
                Some((host, Some(number))) => {
                    let (got, port) = parsed.expect(input);
                    assert_eq!(got, host, "{input}");
                    assert!(
                        matches!(port, Some(PortPart::Number(value)) if value == number),
                        "{input}"
                    );
                }
            }
        }
        let (host, port) = split_host_port("example.test:*").expect("star");
        assert_eq!(host, "example.test");
        assert!(matches!(port, Some(PortPart::Star)));
    }

    #[test]
    fn percent_encoded_paths_decode_before_comparing() {
        assert!(path_part_matches("/%61/b", "/a/b"));
        assert!(path_part_matches("/a%2Fb", "/a%2Fb"));
    }

    #[test]
    fn self_matches_secure_variants() {
        let origin = url("http://example.test/").origin();
        let document_url = url("http://example.test/page");
        assert!(self_matches(
            &url("http://example.test/a"),
            &origin,
            &document_url
        ));
        assert!(self_matches(
            &url("https://example.test/a"),
            &origin,
            &document_url
        ));
        assert!(self_matches(
            &url("ws://example.test/a"),
            &origin,
            &document_url
        ));
        assert!(!self_matches(
            &url("http://other.test/a"),
            &origin,
            &document_url
        ));
    }

    #[test]
    fn base64_value_accepts_both_alphabets_and_padding() {
        assert!(is_base64_value("abc-123_"));
        assert!(is_base64_value("YWJj=="));
        assert!(!is_base64_value("a=b"));
        assert!(!is_base64_value(""));
    }

    #[test]
    fn hash_sources_match_their_digest() {
        // sha256("t1.done();"), the digest the WPT unsafe-hashes fixtures use.
        let expression = "'sha256-wmuLCpoj8EMqfQlPnt5NIMgKkCK62CxAkAiewI0zZps='";
        assert!(hash_matches(expression, "t1.done();"));
        assert!(!hash_matches(expression, "t2.done();"));
    }

    #[test]
    fn strict_dynamic_allows_script_insertion_and_blocks_parser_insertion() {
        let list = vec!["'strict-dynamic'".to_owned()];
        assert!(element_matches(&list, &inline("run();", false)));
        assert!(!element_matches(&list, &inline("run();", true)));
    }

    #[test]
    fn unsafe_hashes_gates_hashes_for_non_script_inline() {
        let hash = "'sha256-wmuLCpoj8EMqfQlPnt5NIMgKkCK62CxAkAiewI0zZps='";
        let attribute = InlineScript {
            source: "t1.done();",
            nonce: "",
            nonceable: false,
            parser_inserted: false,
            kind: InlineKind::ScriptAttribute,
        };
        assert!(!element_matches(&[hash.to_owned()], &attribute));
        assert!(element_matches(
            &["'unsafe-hashes'".to_owned(), hash.to_owned()],
            &attribute
        ));
    }
}
