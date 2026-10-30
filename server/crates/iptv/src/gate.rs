//! Per-source politeness: one token bucket + circuit breaker shared by every HTTP call we make to
//! a provider (catalog, guide, VOD info, stream connects). Xtream panels block abusive clients with
//! HTTP 46x (and 429/5xx under load): after such a response the source is left alone for an
//! exponentially growing time (or the provider's `Retry-After`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Sustained requests per second.
    pub rate_per_sec: f64,
    /// Short bursts allowed.
    pub burst: f64,
    /// First pause after a blocking response; doubles per consecutive failure.
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            rate_per_sec: 2.0,
            burst: 4.0,
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(600),
        }
    }
}

struct State {
    tokens: f64,
    last: Instant,
    penalty_until: Option<Instant>,
    failures: u32,
    trips: u64,
    last_status: Option<u16>,
}

pub struct SourceGate {
    cfg: GateConfig,
    st: Mutex<State>,
}

/// Does this status mean "the provider is pushing back"?
pub fn is_blocking_status(code: u16) -> bool {
    code == 429 || (460..470).contains(&code) || (500..600).contains(&code)
}

impl SourceGate {
    pub fn new(cfg: GateConfig) -> Arc<Self> {
        Arc::new(Self {
            st: Mutex::new(State {
                tokens: cfg.burst,
                last: Instant::now(),
                penalty_until: None,
                failures: 0,
                trips: 0,
                last_status: None,
            }),
            cfg,
        })
    }

    /// Time left of a backoff period, if any.
    pub fn penalty_remaining(&self) -> Option<Duration> {
        let st = self.st.lock().unwrap();
        st.penalty_until
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    /// Number of times the breaker opened.
    pub fn trips(&self) -> u64 {
        self.st.lock().unwrap().trips
    }

    pub fn last_status(&self) -> Option<u16> {
        self.st.lock().unwrap().last_status
    }

    /// Take a token for a request. While a backoff is active the call fails at once when the
    /// remaining pause exceeds `max_wait` (callers that can wait pass a generous value).
    /// `respect_penalty = false` (stream connects) only paces, it never refuses.
    pub async fn acquire(&self, max_wait: Duration, respect_penalty: bool) -> Result<()> {
        loop {
            let wait = {
                let mut st = self.st.lock().unwrap();
                let now = Instant::now();
                if respect_penalty {
                    if let Some(rem) = st.penalty_until.and_then(|t| t.checked_duration_since(now)) {
                        if rem > max_wait {
                            return Err(Error::Unavailable(format!(
                                "the provider is rate limiting us (HTTP {}); paused for another {} s",
                                st.last_status.unwrap_or(0),
                                rem.as_secs().max(1)
                            )));
                        }
                        Some(rem)
                    } else {
                        None
                    }
                } else {
                    None
                }
                .or_else(|| {
                    let el = now.duration_since(st.last).as_secs_f64();
                    st.tokens = (st.tokens + el * self.cfg.rate_per_sec).min(self.cfg.burst);
                    st.last = now;
                    if st.tokens >= 1.0 {
                        st.tokens -= 1.0;
                        None
                    } else {
                        Some(Duration::from_secs_f64(
                            (1.0 - st.tokens) / self.cfg.rate_per_sec,
                        ))
                    }
                })
            };
            match wait {
                None => return Ok(()),
                Some(d) => {
                    if d > max_wait.max(Duration::from_secs(3)) && !respect_penalty {
                        return Ok(());
                    }
                    tokio::time::sleep(d.min(Duration::from_secs(5))).await
                }
            }
        }
    }

    /// Record the answer of the provider.
    pub fn report(&self, status: u16, retry_after: Option<Duration>) {
        let mut st = self.st.lock().unwrap();
        st.last_status = Some(status);
        if is_blocking_status(status) {
            // 5xx on streams is slot lag, not abuse protection: callers only report 46x/429 there
            st.failures += 1;
            let backoff = self
                .cfg
                .backoff_base
                .saturating_mul(1u32 << (st.failures - 1).min(10))
                .min(self.cfg.backoff_max);
            let pause = retry_after
                .unwrap_or(backoff)
                .max(backoff.min(Duration::from_secs(1)));
            let until = Instant::now() + pause.min(self.cfg.backoff_max);
            if st.penalty_until.is_none_or(|t| t < until) {
                st.penalty_until = Some(until);
            }
            st.trips += 1;
            tracing::warn!(
                status,
                pause_s = pause.as_secs(),
                "provider is pushing back; pausing requests to it"
            );
        } else if (200..400).contains(&status) {
            st.failures = 0;
        }
    }
}

/// Registry: one gate per source.
pub struct Gates {
    cfg: GateConfig,
    map: Mutex<HashMap<i64, Arc<SourceGate>>>,
}

impl Gates {
    pub fn new(cfg: GateConfig) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            map: Mutex::default(),
        })
    }

    pub fn get(&self, source_id: i64) -> Arc<SourceGate> {
        self.map
            .lock()
            .unwrap()
            .entry(source_id)
            .or_insert_with(|| SourceGate::new(self.cfg.clone()))
            .clone()
    }
}

/// `Retry-After: <seconds>` of a response.
pub fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paces_requests_after_the_burst() {
        let g = SourceGate::new(GateConfig {
            rate_per_sec: 20.0,
            burst: 2.0,
            ..Default::default()
        });
        let t = Instant::now();
        for _ in 0..6 {
            g.acquire(Duration::from_secs(5), true).await.unwrap();
        }
        // 2 free, 4 more at 20/s ≈ 200 ms
        let e = t.elapsed();
        assert!(
            e >= Duration::from_millis(150) && e < Duration::from_millis(600),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn blocking_statuses_open_the_breaker_with_growing_backoff() {
        let g = SourceGate::new(GateConfig {
            backoff_base: Duration::from_millis(200),
            ..Default::default()
        });
        g.acquire(Duration::ZERO, true).await.unwrap();
        g.report(469, None);
        assert!(g.penalty_remaining().is_some());
        let e = g
            .acquire(Duration::ZERO, true)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("469") && e.contains("rate limiting"), "{e}");
        // streams are paced, not refused
        g.acquire(Duration::ZERO, false).await.unwrap();
        // a caller that can wait gets through once the pause is over
        let t = Instant::now();
        g.acquire(Duration::from_secs(2), true).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(100));
        g.report(469, None); // second failure → longer pause
        assert!(g.penalty_remaining().unwrap() > Duration::from_millis(250));
        assert_eq!(g.trips(), 2);
        // Retry-After wins over our own schedule
        let g2 = SourceGate::new(GateConfig::default());
        g2.report(429, Some(Duration::from_secs(30)));
        assert!(g2.penalty_remaining().unwrap() > Duration::from_secs(20));
        // success resets the failure streak
        g.report(200, None);
        assert_eq!(g.last_status(), Some(200));
    }
}
