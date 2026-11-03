//! HMAC-SHA256 signed, expiring URL capabilities for media endpoints.
//!
//! A signature covers `(scope, user, expiry)`. `scope` names the resource
//! (`direct:12`, `stream:<session>`, `subs:7`, `img:poster:3`), so one signature also covers
//! sub-resources such as HLS segments, and survives changes of non-security parameters
//! (e.g. the image width).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn mac(key: &[u8], scope: &str, user: i64, epoch: i64, exp: i64) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(scope.as_bytes());
    m.update(b"\n");
    m.update(&user.to_be_bytes());
    // bumped on password change / reset / revoke-all / disable: invalidates every URL signed before
    m.update(&epoch.to_be_bytes());
    m.update(&exp.to_be_bytes());
    m
}

/// Signature (base64url) for `(scope, user, epoch, exp)`.
pub fn sign(key: &[u8], scope: &str, user: i64, epoch: i64, exp: i64) -> String {
    URL_SAFE_NO_PAD.encode(mac(key, scope, user, epoch, exp).finalize().into_bytes())
}

/// Constant-time verification including expiry.
pub fn verify(
    key: &[u8],
    scope: &str,
    user: i64,
    epoch: i64,
    exp: i64,
    sig: &str,
    now: i64,
) -> bool {
    if exp < now {
        return false;
    }
    let Ok(raw) = URL_SAFE_NO_PAD.decode(sig) else {
        return false;
    };
    mac(key, scope, user, epoch, exp).verify_slice(&raw).is_ok()
}

/// Signs URLs for one user.
#[derive(Clone)]
pub struct UrlSigner {
    key: std::sync::Arc<[u8]>,
    user: i64,
    epoch: i64,
}

impl UrlSigner {
    pub fn new(key: std::sync::Arc<[u8]>, user: i64, epoch: i64) -> Self {
        Self { key, user, epoch }
    }

    /// Query string (`exp=…&u=…&sig=…`) valid for `ttl` seconds. `exp` is rounded up to a multiple
    /// of `quantum` seconds so repeated calls yield identical (cacheable) URLs.
    pub fn query(&self, scope: &str, ttl: i64, quantum: i64) -> String {
        let now = now_unix();
        let q = quantum.max(1);
        let exp = ((now + ttl + q - 1) / q) * q;
        format!(
            "exp={exp}&u={}&sig={}",
            self.user,
            sign(&self.key, scope, self.user, self.epoch, exp)
        )
    }

    /// `url` with the signature query appended.
    pub fn url(&self, url: &str, scope: &str, ttl: i64, quantum: i64) -> String {
        let sep = if url.contains('?') { '&' } else { '?' };
        format!("{url}{sep}{}", self.query(scope, ttl, quantum))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_tamper_expiry() {
        let key = b"k".repeat(32);
        let now = 1_000;
        let sig = sign(&key, "direct:1", 7, 0, 2_000);
        assert!(verify(&key, "direct:1", 7, 0, 2_000, &sig, now));
        assert!(!verify(&key, "direct:2", 7, 0, 2_000, &sig, now), "scope");
        assert!(!verify(&key, "direct:1", 8, 0, 2_000, &sig, now), "user");
        assert!(!verify(&key, "direct:1", 7, 0, 2_001, &sig, now), "exp");
        assert!(
            !verify(&key, "direct:1", 7, 0, 2_000, &sig, 2_001),
            "expired"
        );
        assert!(
            !verify(&key, "direct:1", 7, 1, 2_000, &sig, now),
            "epoch bumped"
        );
        assert!(!verify(&key, "direct:1", 7, 0, 2_000, "AAAA", now));
        assert!(!verify(
            b"other-key-other-key-other-key-00",
            "direct:1",
            7,
            0,
            2_000,
            &sig,
            now
        ));
    }

    #[test]
    fn quantized_urls_are_stable() {
        let s = UrlSigner::new(vec![1u8; 32].into(), 3, 0);
        assert_eq!(
            s.query("img:poster:1", 3600, 86_400),
            s.query("img:poster:1", 3600, 86_400)
        );
        assert!(s.url("/x?w=1", "a", 60, 1).contains("?w=1&exp="));
    }
}
