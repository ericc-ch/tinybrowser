//! Current browser stack: hyper-util client over hyper-rustls with ring and
//! native roots, the same shape as `crates/net`.
//!
//! `PROBE_CHROME_UA=1` adds a Chrome user agent and header set, so the TLS
//! fingerprint can be separated from header-based bot checks.

use std::time::Instant;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tlsprobe::{ENDPOINTS, ProbeResult, challenge_markers, title, truncate};

const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

#[tokio::main]
async fn main() {
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("native roots")
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let chrome_headers = std::env::var("PROBE_CHROME_UA").is_ok();

    for url in ENDPOINTS {
        let started = Instant::now();
        let mut request = hyper::Request::builder().uri(*url);
        if chrome_headers {
            request = request
                .header("user-agent", CHROME_UA)
                .header(
                    "accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                )
                .header("accept-language", "en-US,en;q=0.9");
        }
        let request = request.body(Empty::<Bytes>::new()).expect("request");
        let result = match client.request(request).await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = match response.into_body().collect().await {
                    Ok(collected) => String::from_utf8_lossy(&collected.to_bytes()).into_owned(),
                    Err(error) => format!("<body error: {error}>"),
                };
                ProbeResult {
                    client: "hyper+rustls/ring",
                    url,
                    status,
                    ms: started.elapsed().as_millis(),
                    challenge: challenge_markers(&body),
                    title: title(&body),
                    body: truncate(&body, 4000),
                }
            }
            Err(error) => ProbeResult {
                client: "hyper+rustls/ring",
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
