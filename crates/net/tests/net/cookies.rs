use net::{Agent, AgentOptions, Method};

/// The global quota is our policy, not the spec's: eviction runs oldest
/// first across all domains. Path scoping, `SameSite`, and security prefixes
/// are WPT's `cookies/` suite; the quota and public-suffix cases below have
/// no WPT coverage.
#[test]
fn cookie_quota_evicts_the_oldest_entries_globally() {
    let agent = Agent::new(AgentOptions::default());
    let oldest = url::Url::parse("https://oldest.example/").expect("oldest URL");
    agent.set_cookie("secure=1; Path=/; Secure", &oldest);
    for domain in 0..60 {
        let domain =
            url::Url::parse(&format!("https://d{domain}.example/")).expect("quota domain URL");
        for cookie in 0..50 {
            agent.set_cookie(&format!("c{cookie}={cookie}; Path=/"), &domain);
        }
    }
    assert_eq!(agent.cookies_for(&oldest), "");
}

/// The per-domain quota: the newest cookie evicts the oldest on that one
/// domain. Our policy; WPT `cookies/` has no quota cases.
#[test]
fn per_domain_quota_keeps_the_newest_fifty() {
    let agent = Agent::new(AgentOptions::default());
    let quota = url::Url::parse("https://quota.example/").expect("quota URL");
    for cookie in 0..51 {
        agent.set_cookie(&format!("n{cookie}={cookie}; Path=/"), &quota);
    }
    let listed = agent.cookies_for(&quota);
    assert_eq!(listed.split("; ").count(), 50);
    assert!(!listed.contains("n0="));
    assert!(listed.contains("n50="));
}

/// Public-suffix domain matching: `Domain=com` and the multi-label
/// `Domain=s3.amazonaws.com` are refused; `github.io` set from its own host
/// stays host-only and never reaches subdomains. WPT `cookies/` has no
/// public-suffix cases.
#[test]
fn public_suffix_domains_are_rejected() {
    let agent = Agent::new(AgentOptions::default());

    let https = url::Url::parse("https://www.example.com/app").expect("https");
    agent.set_cookie("public=1; Path=/; Domain=com", &https);
    assert_eq!(agent.cookies_for(&https), "");

    let s3 = url::Url::parse("https://evil.s3.amazonaws.com/obj").expect("s3");
    agent.set_cookie("bucket=1; Path=/; Domain=s3.amazonaws.com", &s3);
    assert_eq!(agent.cookies_for(&s3), "");

    let pages = url::Url::parse("https://github.io/").expect("pages");
    agent.set_cookie("id=1; Path=/; Domain=github.io", &pages);
    assert_eq!(agent.cookies_for(&pages), "id=1");
    assert_eq!(
        agent.cookies_for(&url::Url::parse("https://foo.github.io/").expect("sub")),
        ""
    );
}

/// `Method::is_safe` decides whether `SameSite=Lax` cookies follow a
/// cross-site top-level navigation. RFC 9110 section 9.2.1 names the safe set.
#[test]
fn get_is_safe() {
    assert!(Method::GET.is_safe());
}

#[test]
fn head_is_safe() {
    assert!(Method::HEAD.is_safe());
}

#[test]
fn options_is_safe() {
    assert!(Method::OPTIONS.is_safe());
}

#[test]
fn trace_is_safe() {
    assert!(Method::parse("TRACE").expect("TRACE is a token").is_safe());
}

#[test]
fn unsafe_methods_are_not_safe() {
    for method in ["POST", "PUT", "DELETE", "PATCH", "CONNECT", "trace"] {
        assert!(
            !Method::parse(method).expect("method is a token").is_safe(),
            "{method}"
        );
    }
}
