//! Chrome-mimicking client: wreq with BoringSSL and a Chrome emulation
//! profile (JA3/JA4 and HTTP/2 fingerprint parity).

use std::time::Instant;

use tlsprobe::{ENDPOINTS, ProbeResult, challenge_markers, title, truncate};
use wreq_util::Emulation;

#[tokio::main]
async fn main() {
    let client = wreq::Client::builder()
        .emulation(Emulation::Chrome149)
        .build()
        .expect("client");
    for url in ENDPOINTS {
        let started = Instant::now();
        let result = match client.get(*url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|error| format!("<body error: {error}>"));
                ProbeResult {
                    client: "wreq+boring (chrome149)",
                    url,
                    status,
                    ms: started.elapsed().as_millis(),
                    challenge: challenge_markers(&body),
                    title: title(&body),
                    body: truncate(&body, 4000),
                }
            }
            Err(error) => ProbeResult {
                client: "wreq+boring (chrome149)",
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
