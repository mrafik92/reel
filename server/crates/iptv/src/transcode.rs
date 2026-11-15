//! Live TV transcodes counted against the playback transcode limits (#13).
//!
//! A live HLS session whose shared packager re-encodes video holds a [`TranscodeSlot`] for as long
//! as its lease. The ledger counts slots per user (every viewer counts against their own
//! `max_transcodes`, joining a running packager included) and distinct packagers server-wide (one
//! ffmpeg each, against the global cap). The playback session manager reads the same counts
//! through [`reel_playback::ExternalTranscodes`], so VOD/library and live transcodes share one
//! budget.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Per-user and global transcode caps for a tune; `0` = unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranscodeLimits {
    pub user_max: usize,
    pub global_max: usize,
}

#[derive(Default)]
struct Ledger {
    by_user: HashMap<i64, usize>,
    /// Packager key → slots on it.
    by_pipe: HashMap<String, usize>,
}

/// Running live transcodes.
#[derive(Default)]
pub(crate) struct LiveTranscodes {
    ledger: Arc<Mutex<Ledger>>,
}

impl LiveTranscodes {
    /// `(this user's slots, packagers with at least one slot)`.
    pub(crate) fn counts(&self, user_id: i64) -> (usize, usize) {
        let l = self.ledger.lock().unwrap();
        (
            l.by_user.get(&user_id).copied().unwrap_or(0),
            l.by_pipe.len(),
        )
    }

    /// The packager `key` already counts server-wide (joining it starts no new ffmpeg).
    pub(crate) fn has_pipe(&self, key: &str) -> bool {
        self.ledger.lock().unwrap().by_pipe.contains_key(key)
    }

    pub(crate) fn reserve(&self, user_id: i64, key: &str) -> TranscodeSlot {
        let mut l = self.ledger.lock().unwrap();
        *l.by_user.entry(user_id).or_default() += 1;
        *l.by_pipe.entry(key.to_string()).or_default() += 1;
        TranscodeSlot {
            ledger: self.ledger.clone(),
            user_id,
            key: key.to_string(),
            released: AtomicBool::new(false),
        }
    }
}

/// One viewer's hold on a video-transcoding live packager. Released on drop, or earlier by
/// [`Self::release`] when the session is stopped.
pub struct TranscodeSlot {
    ledger: Arc<Mutex<Ledger>>,
    user_id: i64,
    key: String,
    released: AtomicBool,
}

impl TranscodeSlot {
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Relaxed)
    }

    pub fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut l = self.ledger.lock().unwrap();
        if let Some(n) = l.by_user.get_mut(&self.user_id) {
            *n -= 1;
            if *n == 0 {
                l.by_user.remove(&self.user_id);
            }
        }
        if let Some(n) = l.by_pipe.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                l.by_pipe.remove(&self.key);
            }
        }
    }
}

impl Drop for TranscodeSlot {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_count_per_user_and_per_packager_and_release_once() {
        let t = LiveTranscodes::default();
        let a = t.reserve(1, "10-transcode-aac");
        let b = t.reserve(2, "10-transcode-aac");
        let c = t.reserve(1, "11-transcode-copy");
        assert_eq!(t.counts(1), (2, 2));
        assert_eq!(t.counts(2), (1, 2));
        assert!(t.has_pipe("10-transcode-aac"));
        a.release();
        a.release();
        drop(a);
        assert_eq!(t.counts(1), (1, 2), "released once, not on drop again");
        drop(b);
        assert_eq!(t.counts(2), (0, 1));
        assert!(!t.has_pipe("10-transcode-aac"));
        drop(c);
        assert_eq!(t.counts(1), (0, 0));
    }
}
