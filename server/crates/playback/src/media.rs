//! Shared media-work resources: the global ffmpeg process budget, the upstream proxy handle and
//! single-flight locks. One `MediaCtx` is shared by the session manager, subtitle conversion and
//! (through the API) remote probes, so *every* ffmpeg/ffprobe subprocess counts toward one limit.

use crate::error::PlaybackError;
use crate::upstream::{Lease, UpstreamProxy};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub struct MediaCtx(Arc<Inner>);

struct Inner {
    procs: Arc<Semaphore>,
    max_procs: usize,
    queue_wait: Duration,
    upstream: Mutex<Option<UpstreamProxy>>,
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Held for the lifetime of one ffmpeg/ffprobe job.
pub type ProcPermit = OwnedSemaphorePermit;

pub const DEFAULT_MAX_FFMPEG_PROCESSES: usize = 6;

impl MediaCtx {
    /// `max_procs == 0` means the default budget.
    pub fn new(max_procs: usize) -> Self {
        Self::with_queue_wait(max_procs, Duration::from_secs(30))
    }

    /// Like [`MediaCtx::new`] with an explicit bound on how long short jobs queue for a slot.
    pub fn with_queue_wait(max_procs: usize, queue_wait: Duration) -> Self {
        let n = if max_procs == 0 {
            DEFAULT_MAX_FFMPEG_PROCESSES
        } else {
            max_procs
        };
        MediaCtx(Arc::new(Inner {
            procs: Arc::new(Semaphore::new(n)),
            max_procs: n,
            queue_wait,
            upstream: Mutex::new(None),
            flights: Mutex::new(HashMap::new()),
        }))
    }

    /// How long short jobs (subtitle extraction, probes) wait for a process slot before `Busy`.
    pub fn queue_wait(&self) -> Duration {
        self.0.queue_wait
    }

    pub fn max_processes(&self) -> usize {
        self.0.max_procs
    }

    /// ffmpeg/ffprobe jobs currently running or reserved.
    pub fn processes_in_use(&self) -> usize {
        self.0.max_procs - self.0.procs.available_permits()
    }

    pub fn set_upstream(&self, p: UpstreamProxy) {
        *self.0.upstream.lock().unwrap() = Some(p);
    }

    pub fn upstream(&self) -> Option<UpstreamProxy> {
        self.0.upstream.lock().unwrap().clone()
    }

    /// Reserve a process slot immediately or fail (sessions).
    pub fn try_proc(&self) -> Result<ProcPermit, PlaybackError> {
        self.0
            .procs
            .clone()
            .try_acquire_owned()
            .map_err(|_| PlaybackError::GlobalLimit(self.0.max_procs))
    }

    /// Wait up to `wait` for a slot (bounded queue for short jobs: subtitle extraction, probes).
    pub async fn proc(&self, wait: Duration) -> Result<ProcPermit, PlaybackError> {
        match tokio::time::timeout(wait, self.0.procs.clone().acquire_owned()).await {
            Ok(Ok(p)) => Ok(p),
            _ => Err(PlaybackError::Busy),
        }
    }

    /// Lock serialising identical work (`key`), e.g. the same subtitle conversion.
    pub fn flight(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.0.flights.lock().unwrap();
        if m.len() > 1024 {
            m.retain(|_, v| Arc::strong_count(v) > 1);
        }
        m.entry(key.to_string()).or_default().clone()
    }

    /// Input argument for a subprocess: local paths pass through, remote (provider) URLs are
    /// replaced by a loopback proxy URL so the subprocess never sees the real URL or credentials.
    /// The returned lease keeps the token alive; drop it when the job ends.
    pub fn proxied_input(
        &self,
        path: &str,
        ua: &str,
        ttl: Duration,
    ) -> Result<(String, Option<Lease>), PlaybackError> {
        if !crate::ffmpeg::is_remote(path) {
            return Ok((path.to_string(), None));
        }
        let p = self.upstream().ok_or_else(|| {
            PlaybackError::Invalid("remote inputs need the upstream proxy".into())
        })?;
        let lease = p.lease(path, ua, ttl)?;
        Ok((lease.url().to_string(), Some(lease)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn budget_is_shared_and_bounded() {
        let m = MediaCtx::new(2);
        let a = m.try_proc().unwrap();
        let _b = m.try_proc().unwrap();
        assert!(matches!(m.try_proc(), Err(PlaybackError::GlobalLimit(2))));
        assert!(matches!(
            m.proc(Duration::from_millis(30)).await,
            Err(PlaybackError::Busy)
        ));
        drop(a);
        assert!(m.proc(Duration::from_millis(30)).await.is_ok());
    }

    #[test]
    fn local_inputs_pass_remote_needs_proxy() {
        let m = MediaCtx::new(1);
        assert_eq!(
            m.proxied_input("/m/a.mkv", "ua", Duration::from_secs(1))
                .unwrap()
                .0,
            "/m/a.mkv"
        );
        assert!(
            m.proxied_input("http://prov/x.mkv", "ua", Duration::from_secs(1))
                .is_err()
        );
    }
}
