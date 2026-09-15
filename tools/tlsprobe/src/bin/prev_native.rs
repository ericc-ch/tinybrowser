//! v1's client stack: ureq 3 with native-tls (system OpenSSL).
//!
//! `PROBE_CHROME_UA=1` adds a Chrome user agent and header set, so the TLS
//! fingerprint can be separated from header-based bot checks.

use std::time::Instant;

use tlsprobe::{ENDPOINTS, ProbeResult, challenge_markers, title, truncate};

const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

fn main() {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build()
        .new_agent();
    let chrome_headers = std::env::var("PROBE_CHROME_UA").is_ok();
    for url in ENDPOINTS {
        let started = Instant::now();
        let mut request = agent.get(*url);
        if chrome_headers {
            request = request
                .header("user-agent", CHROME_UA)
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "en-US,en;q=0.9");
        }
        let result = match request.call() {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let body = response
                    .body_mut()
                    .read_to_string()
                    .unwrap_or_else(|error| format!("<body error: {error}>"));
                ProbeResult {
                    client: "ureq+native-tls",
                    url,
                    status,
                    ms: started.elapsed().as_millis(),
                    challenge: challenge_markers(&body),
                    title: title(&body),
                    body: truncate(&body, 4000),
                }
            }
            Err(error) => ProbeResult {
                client: "ureq+native-tls",
                url,
                status: 0,
                ms: started.elapsed().as_millis(),
                challenge: Vec::new(),
                title: None,
                body: format!("<error: {error}>"),
            },
        };
        println!("{}", serde_json::to_string(&result).expect("serialize"));
    }
}
