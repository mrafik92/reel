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

/// Hard bound of tracked keys.
const MAX_ENTRIES: usize = 20_000;

struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
    last: Instant,
}

pub struct RateLimiter {
    cfg: LimiterConfig,
    map: Mutex<HashMap<String, Entry>>,
    last_sweep: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(cfg: LimiterConfig) -> Self {
        Self {
            cfg,
            map: Mutex::default(),
            last_sweep: Mutex::new(Instant::now()),
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

    /// Atomically check the key and count an attempt *before* it is verified (so parallel guesses
    /// cannot exceed the window). Give the attempt back with [`refund`](Self::refund) if it
    /// turns out to be legitimate. `Err(retry_after)` while locked (nothing is counted).
    pub fn reserve(&self, key: &str, threshold: u32) -> Result<(), Duration> {
        let now = Instant::now();
        {
            let map = self.map.lock().unwrap();
            if let Some(until) = map.get(key).and_then(|e| e.locked_until) {
                if until > now {
                    return Err(until - now);
                }
            }
        }
        self.fail(key, threshold);
        Ok(())
    }

    /// Undo one [`reserve`](Self::reserve) (successful login).
    pub fn refund(&self, key: &str, threshold: u32) {
        let mut map = self.map.lock().unwrap();
        if let Some(e) = map.get_mut(key) {
            e.failures = e.failures.saturating_sub(1);
            if e.failures < threshold {
                e.locked_until = None;
            }
            if e.failures == 0 {
                map.remove(key);
            }
        }
    }

    /// Failures currently counted for `key`.
    pub fn failures(&self, key: &str) -> u32 {
        self.map.lock().unwrap().get(key).map_or(0, |e| e.failures)
    }

    /// Count a failure without ever locking (soft, global per-username pressure).
    pub fn soft_fail(&self, key: &str) -> u32 {
        self.fail(key, u32::MAX);
        self.failures(key)
    }

    /// Entries held (bounded by `MAX_ENTRIES`).
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Record a failure; locks once `failures >= threshold`.
    pub fn fail(&self, key: &str, threshold: u32) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap();
        if map.len() >= MAX_ENTRIES / 2 {
            // decay sweep at most once a second (it is O(n))
            let mut sw = self.last_sweep.lock().unwrap();
            if now.duration_since(*sw) >= Duration::from_secs(1) {
                *sw = now;
                let decay = self.cfg.decay;
                map.retain(|_, e| now.duration_since(e.last) < decay);
            }
        }
        if map.len() >= MAX_ENTRIES && !map.contains_key(key) {
            // still full of live entries: drop the oldest tenth (attackers rotating keys cannot
            // grow memory without bound)
            let mut by_age: Vec<(String, Instant)> =
                map.iter().map(|(k, e)| (k.clone(), e.last)).collect();
            by_age.sort_by_key(|(_, t)| *t);
            for (k, _) in by_age.into_iter().take(MAX_ENTRIES / 10) {
                map.remove(&k);
            }
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
