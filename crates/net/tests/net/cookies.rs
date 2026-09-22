use net::{Agent, AgentOptions};

/// The jar's global quota is our policy, not the spec's: eviction runs oldest
/// first across all domains. Cookie *rules* (path scoping, `SameSite`,
/// security prefixes, public suffixes) are WPT's `cookies/` suite.
#[test]
fn cookie_quota_evicts_the_oldest_entries_globally() {
    let agent = Agent::new(AgentOptions::default()).expect("default options are valid");
    let oldest = url::Url::parse("https://oldest.example/").expect("oldest URL");
    agent.set_cookie("secure=1; Path=/; Secure", &oldest);
    for domain in 0..60 {
        let domain =
            url::Url::parse(&format!("https://d{domain}.example/")).expect("quota domain URL");
        for cookie in 0..50 {
            agent.set_cookie(&format!("c{cookie}={cookie}; Path=/"), &domain);
        }
    }
    assert!(agent.cookies_for(&oldest).is_empty());
}
