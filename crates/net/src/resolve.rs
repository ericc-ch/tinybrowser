use std::net::Ipv4Addr;
use std::sync::Arc;

use crate::error::{NetError, ProtocolError};

/// One `--resolve=PATTERN=ADDR` rule. First match wins.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Rule {
    pattern: Box<str>,
    target: Target,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Addr(Ipv4Addr),
    Fail,
}

/// Ordered host rewrites. Empty means libc DNS. Cheap to clone.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostMap {
    rules: Arc<Vec<Rule>>,
}

impl HostMap {
    pub(crate) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub(crate) fn with_spec(&self, spec: &str) -> Result<Self, NetError> {
        let mut rules = (*self.rules).clone();
        rules.push(parse_rule(spec)?);
        Ok(Self {
            rules: Arc::new(rules),
        })
    }

    pub(crate) fn lookup(&self, host: &str) -> Option<Mapped> {
        if self.rules.is_empty() {
            return None;
        }
        let host = host.to_ascii_lowercase();
        for rule in self.rules.iter() {
            if glob_match(&rule.pattern, &host) {
                return Some(match rule.target {
                    Target::Fail => Mapped::Fail,
                    Target::Addr(ip) => Mapped::Addr(ip),
                });
            }
        }
        None
    }
}

pub(crate) enum Mapped {
    Addr(Ipv4Addr),
    Fail,
}

fn parse_rule(spec: &str) -> Result<Rule, NetError> {
    let Some((pattern, addr)) = spec.split_once('=') else {
        return Err(invalid_resolve());
    };
    if pattern.is_empty() || addr.is_empty() {
        return Err(invalid_resolve());
    }
    let pattern = pattern.to_ascii_lowercase().into_boxed_str();
    let target = if addr.eq_ignore_ascii_case("fail") {
        Target::Fail
    } else {
        let ip = addr.parse::<Ipv4Addr>().map_err(|_| invalid_resolve())?;
        Target::Addr(ip)
    };
    Ok(Rule { pattern, target })
}

fn glob_match(pattern: &str, host: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(after_first) = host.strip_prefix(first) else {
        return false;
    };
    let Some(last) = parts.next_back() else {
        return after_first.is_empty();
    };
    let Some(mut rest) = after_first.strip_suffix(last) else {
        return false;
    };
    for part in parts {
        match rest.find(part) {
            None => return false,
            Some(index) => rest = &rest[index + part.len()..],
        }
    }
    true
}

fn invalid_resolve() -> NetError {
    NetError::Protocol(ProtocolError::InvalidResolve)
}
