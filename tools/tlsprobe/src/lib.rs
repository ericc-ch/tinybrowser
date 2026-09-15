//! Shared helpers for the TLS stack probes.

/// Endpoints used by every probe.
pub const ENDPOINTS: &[&str] = &[
    "https://tls.peet.ws/api/all",
    "https://tls.browserleaks.com/json",
    "https://httpbin.org/get",
    "https://nowsecure.nl/",
    "https://nopecha.com/demo/cloudflare",
    "https://www.g2.com/",
    "https://www.reddit.com/",
];

/// One probe result line.
#[derive(serde::Serialize)]
pub struct ProbeResult {
    pub client: &'static str,
    pub url: &'static str,
    pub status: u16,
    pub ms: u128,
    /// Challenge markers found in the body, if any.
    pub challenge: Vec<&'static str>,
    /// Document title, for challenge interstitials.
    pub title: Option<String>,
    /// First bytes of the body, enough to read a JA3/JA4 JSON reply.
    pub body: String,
}

/// First `<title>` in the body.
#[must_use]
pub fn title(body: &str) -> Option<String> {
    let lowered = body.to_ascii_lowercase();
    let start = lowered.find("<title")?;
    let open_end = lowered[start..].find('>')? + start + 1;
    let end = lowered[open_end..].find("</title>")? + open_end;
    Some(body[open_end..end].trim().to_owned())
}

const CHALLENGE_MARKERS: &[&str] = &[
    "Just a moment",
    "cf-chl",
    "Attention Required",
    "Enable JavaScript and cookies",
    "challenge-platform",
];

#[must_use]
pub fn challenge_markers(body: &str) -> Vec<&'static str> {
    CHALLENGE_MARKERS
        .iter()
        .copied()
        .filter(|marker| body.contains(marker))
        .collect()
}

#[must_use]
pub fn truncate(body: &str, limit: usize) -> String {
    if body.len() <= limit {
        return body.to_owned();
    }
    let mut end = limit;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &body[..end])
}
