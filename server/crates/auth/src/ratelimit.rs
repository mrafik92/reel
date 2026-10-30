//! In-memory failure counters with exponential lockout, keyed by arbitrary strings
//! (`ip:1.2.3.4`, `user:alice`).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct LimiterConfig {
    /// Failures per username before lockout starts.
    pub user_max_failures: u32,
    /// Failures per client IP before lockout starts.
    pub ip_max_failures: u32,
    /// First lock duration; doubles with every further failure.
    pub base_lock: Duration,
    pub max_lock: Duration,
    /// A key with no failures for this long is forgotten.
    pub decay: Duration,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            user_max_failures: 5,
            ip_max_failures: 20,
            base_lock: Duration::from_secs(15),
            max_lock: Duration::from_secs(15 * 60),
            decay: Duration::from_secs(15 * 60),
        }
    }
}

struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
    last: Instant,
}

pub struct RateLimiter {
    cfg: LimiterConfig,
    map: Mutex<HashMap<String, Entry>>,
}

impl RateLimiter {
    pub fn new(cfg: LimiterConfig) -> Self {
        Self {
            cfg,
            map: Mutex::default(),
        }
    }

    pub fn config(&self) -> &LimiterConfig {
        &self.cfg
    }

    /// `Err(retry_after)` while `key` is locked.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        let now = Instant::now();
        let map = self.map.lock().unwrap();
        match map.get(key).and_then(|e| e.locked_until) {
            Some(until) if until > now => Err(until - now),
            _ => Ok(()),
        }
    }

    /// Record a failure; locks once `failures >= threshold`.
    pub fn fail(&self, key: &str, threshold: u32) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap();
        if map.len() > 10_000 {
            let decay = self.cfg.decay;
            map.retain(|_, e| now.duration_since(e.last) < decay);
        }
        let e = map.entry(key.to_string()).or_insert(Entry {
            failures: 0,
            locked_until: None,
            last: now,
        });
        if now.duration_since(e.last) > self.cfg.decay {
            e.failures = 0;
        }
        e.failures += 1;
        e.last = now;
        if e.failures >= threshold {
            let exp = (e.failures - threshold).min(20);
            let lock = self
                .cfg
                .base_lock
                .saturating_mul(1u32 << exp)
                .min(self.cfg.max_lock);
            e.locked_until = Some(now + lock);
        }
    }

    pub fn fail_user(&self, key: &str) {
        self.fail(key, self.cfg.user_max_failures);
    }

    pub fn fail_ip(&self, key: &str) {
        self.fail(key, self.cfg.ip_max_failures);
    }

    /// Forget a key (successful login for the username key).
    pub fn clear(&self, key: &str) {
        self.map.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_threshold_with_backoff_and_clears() {
        let l = RateLimiter::new(LimiterConfig {
            user_max_failures: 2,
            base_lock: Duration::from_millis(40),
            max_lock: Duration::from_millis(200),
            ..Default::default()
        });
        l.fail_user("u");
        assert!(l.check("u").is_ok());
        l.fail_user("u");
        let first = l.check("u").unwrap_err();
        assert!(first <= Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(50));
        assert!(l.check("u").is_ok());
        l.fail_user("u"); // third failure: 2x base
        let second = l.check("u").unwrap_err();
        assert!(second > Duration::from_millis(40));
        assert!(l.check("other").is_ok());
        l.clear("u");
        assert!(l.check("u").is_ok());
    }
}
