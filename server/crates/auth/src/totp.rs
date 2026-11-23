//! TOTP (RFC 6238: HMAC-SHA1, 6 digits, 30 s) and encryption of its secret at rest.

use rand::RngCore;
use ring::{aead, hkdf, hmac};

pub const STEP_SECS: i64 = 30;
const SECRET_BYTES: usize = 20;
const NONCE_BYTES: usize = 12;
/// Steps accepted on each side of now, for clock drift between the phone and the server.
const DRIFT_STEPS: i64 = 1;

pub fn new_secret() -> Vec<u8> {
    let mut s = vec![0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut s);
    s
}

/// RFC 4648 base32 without padding, the form authenticator apps expect.
pub fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(data.len() * 8 / 5 + 1);
    let (mut buf, mut bits) = (0u32, 0u32);
    for b in data {
        buf = (buf << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 31) as usize] as char);
        }
        buf &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The `otpauth://` link shown as a QR code. Usernames are limited to `[A-Za-z0-9._-]`, so the label
/// needs no escaping.
pub fn otpauth_uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    format!(
        "otpauth://totp/{issuer}:{account}?secret={}&issuer={issuer}&algorithm=SHA1&digits=6&period={STEP_SECS}",
        base32(secret)
    )
}

fn code_at(secret: &[u8], step: i64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let d = tag.as_ref();
    let i = (d[d.len() - 1] & 0x0f) as usize;
    (u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) & 0x7fff_ffff) % 1_000_000
}

/// The step `code` is valid for, if it is within the drift window and newer than `after_step`.
/// Every candidate is compared, so timing doesn't reveal which one matched.
pub fn matching_step(secret: &[u8], code: &str, now: i64, after_step: i64) -> Option<i64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let presented: u32 = code.parse().ok()?;
    let current = now.div_euclid(STEP_SECS);
    let mut found = None;
    for step in current - DRIFT_STEPS..=current + DRIFT_STEPS {
        if code_at(secret, step) ^ presented == 0 && step > after_step {
            found = Some(step);
        }
    }
    found
}

fn sealing_key(signing_key: &[u8]) -> Option<aead::LessSafeKey> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"reel-totp-secret-v1").extract(signing_key);
    let okm = prk
        .expand(&[b"chacha20-poly1305"], &aead::CHACHA20_POLY1305)
        .ok()?;
    Some(aead::LessSafeKey::new(aead::UnboundKey::from(okm)))
}

/// Encrypt a TOTP secret, bound to its user: a row copied onto another account won't open.
pub fn seal(signing_key: &[u8], user_id: i64, secret: &[u8]) -> Option<Vec<u8>> {
    let key = sealing_key(signing_key)?;
    let mut nonce = [0u8; NONCE_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut body = secret.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(user_id.to_be_bytes()),
        &mut body,
    )
    .ok()?;
    let mut out = nonce.to_vec();
    out.append(&mut body);
    Some(out)
}

pub fn open(signing_key: &[u8], user_id: i64, sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() <= NONCE_BYTES {
        return None;
    }
    let key = sealing_key(signing_key)?;
    let (nonce, body) = sealed.split_at(NONCE_BYTES);
    let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut body = body.to_vec();
    let plain = key
        .open_in_place(nonce, aead::Aad::from(user_id.to_be_bytes()), &mut body)
        .ok()?;
    Some(plain.to_vec())
}

/// The current code, for tests that act as the authenticator app.
#[doc(hidden)]
pub fn code_for_tests(secret: &[u8], now: i64) -> String {
    format!("{:06}", code_at(secret, now.div_euclid(STEP_SECS)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc_6238_vectors() {
        // the RFC's 8-digit values, truncated to the low 6 digits
        for (t, code) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
        ] {
            assert_eq!(code_at(RFC_SECRET, t / STEP_SECS), code, "t={t}");
        }
    }

    #[test]
    fn base32_matches_rfc_4648() {
        assert_eq!(base32(b""), "");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(base32(b"fo"), "MZXQ");
        assert_eq!(base32(b"foo"), "MZXW6");
        assert_eq!(base32(b"foob"), "MZXW6YQ");
        assert_eq!(base32(b"fooba"), "MZXW6YTB");
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn a_code_is_accepted_within_the_window_and_only_once() {
        let now = 1_700_000_015;
        let code = code_for_tests(RFC_SECRET, now);
        let step = now / STEP_SECS;
        assert_eq!(matching_step(RFC_SECRET, &code, now, 0), Some(step));
        assert_eq!(
            matching_step(
                RFC_SECRET,
                &code[..3]
                    .to_string()
                    .chars()
                    .chain(code[3..].chars())
                    .collect::<String>(),
                now + STEP_SECS,
                0
            ),
            Some(step),
            "one step of drift"
        );
        assert_eq!(
            matching_step(RFC_SECRET, &code, now + 2 * STEP_SECS, 0),
            None
        );
        assert_eq!(
            matching_step(RFC_SECRET, &code, now, step),
            None,
            "replayed"
        );
    }

    #[test]
    fn malformed_codes_are_refused() {
        for bad in ["", "12345", "1234567", "12a456", "١٢٣٤٥٦"] {
            assert_eq!(
                matching_step(RFC_SECRET, bad, 1_700_000_000, 0),
                None,
                "{bad}"
            );
        }
    }

    #[test]
    fn the_sealed_secret_opens_only_for_its_user_and_key() {
        let key = [7u8; 32];
        let sealed = seal(&key, 42, RFC_SECRET).unwrap();
        assert!(!sealed.windows(RFC_SECRET.len()).any(|w| w == RFC_SECRET));
        assert_eq!(open(&key, 42, &sealed).unwrap(), RFC_SECRET);
        assert!(open(&key, 43, &sealed).is_none(), "another user");
        assert!(open(&[8u8; 32], 42, &sealed).is_none(), "another key");
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, 42, &tampered).is_none());
        assert!(open(&key, 42, &sealed[..5]).is_none());
    }
}
