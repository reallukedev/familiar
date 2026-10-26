//! In-memory sliding-window limits keyed by IP or account. Lost on restart, which is
//! fine: accounts also lock in the database after repeated failures.
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// A named limit: at most `max` events per `window`.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Events allowed per window.
    pub max: usize,
    /// Window length.
    pub window: Duration,
}

/// Sign-in attempts per IP.
pub const LOGIN_IP: Rule = Rule {
    max: 20,
    window: Duration::from_secs(600),
};
/// TOTP attempts per challenge or session.
pub const TOTP: Rule = Rule {
    max: 6,
    window: Duration::from_secs(300),
};
/// OAuth client registrations per IP.
pub const REGISTER_IP: Rule = Rule {
    max: 10,
    window: Duration::from_secs(3600),
};
/// Token endpoint calls per IP.
pub const TOKEN_IP: Rule = Rule {
    max: 120,
    window: Duration::from_secs(60),
};
/// Failed bearer tokens per IP on /mcp.
pub const BEARER_FAIL_IP: Rule = Rule {
    max: 30,
    window: Duration::from_secs(300),
};

/// Rate-limit key for an address: IPv6 is grouped by /64, since one host usually owns a
/// whole /64 and could otherwise rotate addresses freely.
pub fn ip_key(ip: std::net::IpAddr) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

/// OAuth authorization requests per IP.
pub const AUTHORIZE_IP: Rule = Rule {
    max: 60,
    window: Duration::from_secs(600),
};

/// Shared limiter.
#[derive(Default)]
pub struct Limiter {
    hits: Mutex<HashMap<(&'static str, String), Vec<Instant>>>,
}

impl Limiter {
    /// Record an event; `Err(seconds)` when over the limit (the event is not recorded).
    pub fn hit(&self, name: &'static str, key: &str, rule: Rule) -> Result<(), u64> {
        self.hit_at(name, key, rule, Instant::now())
    }

    /// Check without recording.
    pub fn check(&self, name: &'static str, key: &str, rule: Rule) -> Result<(), u64> {
        let now = Instant::now();
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let list = hits.entry((name, key.to_owned())).or_default();
        list.retain(|t| now.duration_since(*t) < rule.window);
        Self::verdict(list, rule, now)
    }

    fn verdict(list: &[Instant], rule: Rule, now: Instant) -> Result<(), u64> {
        if list.len() >= rule.max {
            let oldest = list.first().copied().unwrap_or(now);
            let wait = rule.window.saturating_sub(now.duration_since(oldest));
            return Err(wait.as_secs().max(1));
        }
        Ok(())
    }

    fn hit_at(&self, name: &'static str, key: &str, rule: Rule, now: Instant) -> Result<(), u64> {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if hits.len() > 50_000 {
            // Bounded memory under a flood: drop expired keys.
            hits.retain(|_, v| {
                v.last()
                    .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(3600))
            });
        }
        let list = hits.entry((name, key.to_owned())).or_default();
        list.retain(|t| now.duration_since(*t) < rule.window);
        Self::verdict(list, rule, now)?;
        list.push(now);
        Ok(())
    }

    /// Forget a key (after a successful sign-in).
    pub fn reset(&self, name: &'static str, key: &str) {
        let mut hits = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        hits.remove(&(name, key.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_slides_and_keys_are_independent() {
        let l = Limiter::default();
        let rule = Rule {
            max: 2,
            window: Duration::from_secs(10),
        };
        let t0 = Instant::now();
        assert!(l.hit_at("x", "a", rule, t0).is_ok());
        assert!(l.hit_at("x", "a", rule, t0).is_ok());
        let wait = l
            .hit_at("x", "a", rule, t0 + Duration::from_secs(4))
            .unwrap_err();
        assert!((5..=6).contains(&wait));
        assert!(l.hit_at("x", "b", rule, t0).is_ok());
        assert!(l.hit_at("y", "a", rule, t0).is_ok());
        assert!(
            l.hit_at("x", "a", rule, t0 + Duration::from_secs(11))
                .is_ok()
        );
        l.reset("x", "a");
        assert!(l.check("x", "a", rule).is_ok());
        let a: std::net::IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: std::net::IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        assert_eq!(ip_key(a), ip_key(b), "one /64 is one key");
        assert_eq!(ip_key("::ffff:1.2.3.4".parse().unwrap()), "1.2.3.4");
    }
}
