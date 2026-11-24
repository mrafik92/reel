//! TV sign-in by code (#147), the OAuth device grant (RFC 8628) done inside Reel: the TV shows a
//! short code, a signed-in phone approves it, and the TV's poll receives its own session.

use std::{collections::HashMap, sync::Mutex, time::Instant};

use super::*;

const MAX_PAIRINGS: usize = 1_000;
/// Pairings one address may start before the limiter slows it down.
const STARTS_PER_IP: u32 = 20;
/// Wrong codes one signed-in user may try before the limiter slows them down.
const LOOKUPS_PER_USER: u32 = 10;
/// RFC 8628's alphabet: no vowels (no accidental words) and no look-alike characters.
const USER_CODE_ALPHABET: &[u8; 20] = b"BCDFGHJKLMNPQRSTVWXZ";

/// What the TV shows and polls with.
#[derive(Debug, Clone)]
pub struct DevicePairing {
    /// Secret the TV polls with; never displayed.
    pub device_code: String,
    /// Short code the person checks on their phone, `BCDF-GHJK`.
    pub user_code: String,
    pub expires_in_s: u64,
    pub interval_s: u64,
}

/// What the approver sees before saying yes.
#[derive(Debug, Clone)]
pub struct PairingInfo {
    pub device_name: Option<String>,
    pub client: Option<String>,
    pub ip: Option<String>,
    pub expires_in_s: u64,
}

struct Pairing {
    device_hash: String,
    dev: Device,
    expires: Instant,
    last_poll: Option<Instant>,
    approved_by: Option<i64>,
}

#[derive(Default)]
struct Pairings {
    /// By normalized user code.
    by_code: HashMap<String, Pairing>,
    /// Device-code hash → user code.
    by_device: HashMap<String, String>,
}

impl Pairings {
    fn remove(&mut self, code: &str) -> Option<Pairing> {
        let p = self.by_code.remove(code)?;
        self.by_device.remove(&p.device_hash);
        Some(p)
    }

    fn sweep(&mut self) {
        let now = Instant::now();
        let gone: Vec<String> = self
            .by_code
            .iter()
            .filter(|(_, p)| p.expires <= now)
            .map(|(c, _)| c.clone())
            .collect();
        for c in gone {
            self.remove(&c);
        }
    }
}

#[derive(Default)]
pub(super) struct DeviceState(Mutex<Pairings>);

fn new_user_code() -> String {
    let mut out = String::with_capacity(8);
    while out.len() < 8 {
        let mut b = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut b);
        // below 240, `% 20` is uniform
        for x in b.into_iter().filter(|x| *x < 240) {
            if out.len() < 8 {
                out.push(USER_CODE_ALPHABET[usize::from(x % 20)] as char);
            }
        }
    }
    out
}

fn normalize_user_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_uppercase())
        .take(9)
        .collect()
}

/// The TV names itself, so its text is shown to the approver only after being bounded and cleaned.
fn clean_label(s: Option<String>, max: usize) -> Option<String> {
    // zero-width and bidi-override characters could make a device name read as something else
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
    };
    s.map(|v| {
        v.chars()
            .filter(|c| !hidden(*c))
            .take(max)
            .collect::<String>()
            .trim()
            .to_string()
    })
    .filter(|v| !v.is_empty())
}

impl AuthService {
    /// Start a pairing for a device that isn't signed in.
    pub fn device_start(&self, mut dev: Device) -> AuthResult<DevicePairing> {
        let ip_key = format!("devstart:{}", ip_bucket(dev.ip.as_deref()));
        self.limiter
            .reserve(&ip_key, STARTS_PER_IP)
            .map_err(AuthError::Locked)?;
        dev.device_id = clean_label(dev.device_id, 128);
        dev.device_name = clean_label(dev.device_name, 64);
        dev.client = clean_label(dev.client, 32);
        let device_code = random_b64(32);
        let device_hash = hash_token(&device_code);
        let ttl = self.opts.device_code_ttl;
        let mut m = self.devices.0.lock().unwrap();
        if m.by_code.len() >= MAX_PAIRINGS {
            m.sweep();
            if m.by_code.len() >= MAX_PAIRINGS {
                return Err(AuthError::Locked(Duration::from_secs(10)));
            }
        }
        let code = loop {
            let c = new_user_code();
            if !m.by_code.contains_key(&c) {
                break c;
            }
        };
        m.by_device.insert(device_hash.clone(), code.clone());
        m.by_code.insert(
            code.clone(),
            Pairing {
                device_hash,
                dev,
                expires: Instant::now() + ttl,
                last_poll: None,
                approved_by: None,
            },
        );
        Ok(DevicePairing {
            device_code,
            user_code: format!("{}-{}", &code[..4], &code[4..]),
            expires_in_s: ttl.as_secs(),
            interval_s: self.opts.device_poll_interval.as_secs(),
        })
    }

    /// The TV's poll: `None` while waiting. The session is handed out exactly once.
    pub async fn device_poll(&self, device_code: &str) -> AuthResult<Option<LoginOutcome>> {
        let hash = hash_token(device_code);
        let approved = {
            let mut m = self.devices.0.lock().unwrap();
            let code = m.by_device.get(&hash).cloned().ok_or(AuthError::NotFound)?;
            let now = Instant::now();
            let interval = self.opts.device_poll_interval;
            let p = m.by_code.get_mut(&code).ok_or(AuthError::NotFound)?;
            if p.expires <= now {
                m.remove(&code);
                return Err(AuthError::NotFound);
            }
            if let Some(last) = p.last_poll {
                let since = now.duration_since(last);
                if since < interval {
                    return Err(AuthError::Locked(interval - since));
                }
            }
            p.last_poll = Some(now);
            if p.approved_by.is_none() {
                return Ok(None);
            }
            m.remove(&code)
        };
        let Some(Pairing {
            dev,
            approved_by: Some(user_id),
            ..
        }) = approved
        else {
            return Err(AuthError::NotFound);
        };
        // access may have been withdrawn between the approval and this poll
        let user = match self.get_user(user_id).await? {
            Some(u) if !u.disabled => u,
            _ => return Err(AuthError::NotFound),
        };
        let (token, expires_at) = self.create_session(user.id, &dev).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "login_device",
            None,
            json!({"device": dev.device_name, "client": dev.client}),
            dev.ip.as_deref(),
        )
        .await;
        Ok(Some(LoginOutcome {
            token,
            user,
            expires_at,
            password_weak: false,
        }))
    }

    /// Count a code attempt by a signed-in user; a hit is given back.
    fn pairing_attempt(&self, user_id: i64, code: &str) -> AuthResult<String> {
        let key = format!("devlink:{user_id}");
        self.limiter
            .reserve(&key, LOOKUPS_PER_USER)
            .map_err(AuthError::Locked)?;
        let code = normalize_user_code(code);
        let m = self.devices.0.lock().unwrap();
        match m.by_code.get(&code) {
            Some(p) if p.expires > Instant::now() => {
                self.limiter.refund(&key, LOOKUPS_PER_USER);
                Ok(code)
            }
            _ => Err(AuthError::NotFound),
        }
    }

    pub fn device_lookup(&self, user_id: i64, code: &str) -> AuthResult<PairingInfo> {
        let code = self.pairing_attempt(user_id, code)?;
        let m = self.devices.0.lock().unwrap();
        let p = m.by_code.get(&code).ok_or(AuthError::NotFound)?;
        if p.approved_by.is_some() {
            return Err(AuthError::Conflict("this code was already approved".into()));
        }
        Ok(PairingInfo {
            device_name: p.dev.device_name.clone(),
            client: p.dev.client.clone(),
            ip: p.dev.ip.clone(),
            expires_in_s: p
                .expires
                .saturating_duration_since(Instant::now())
                .as_secs(),
        })
    }

    /// Approve: the device's next poll gets a session for `user`.
    pub async fn device_approve(&self, user: &User, code: &str) -> AuthResult<()> {
        let code = self.pairing_attempt(user.id, code)?;
        let (name, client) = {
            let mut m = self.devices.0.lock().unwrap();
            let p = m.by_code.get_mut(&code).ok_or(AuthError::NotFound)?;
            if p.approved_by.is_some() {
                return Err(AuthError::Conflict("this code was already approved".into()));
            }
            p.approved_by = Some(user.id);
            (p.dev.device_name.clone(), p.dev.client.clone())
        };
        self.audit(
            Some(user.id),
            Some(&user.username),
            "device_link_approve",
            None,
            json!({"device": name, "client": client}),
            None,
        )
        .await;
        Ok(())
    }

    /// Refuse: the code stops working and the device has to start over.
    pub fn device_deny(&self, user_id: i64, code: &str) -> AuthResult<()> {
        let code = self.pairing_attempt(user_id, code)?;
        let mut m = self.devices.0.lock().unwrap();
        match m.by_code.get(&code) {
            Some(p) if p.approved_by.is_none_or(|u| u == user_id) => {
                m.remove(&code);
                Ok(())
            }
            _ => Err(AuthError::NotFound),
        }
    }

    /// Approvals not yet collected die with the approver's access.
    pub(super) fn drop_device_approvals(&self, user_id: i64) {
        let mut m = self.devices.0.lock().unwrap();
        let gone: Vec<String> = m
            .by_code
            .iter()
            .filter(|(_, p)| p.approved_by == Some(user_id))
            .map(|(c, _)| c.clone())
            .collect();
        for c in gone {
            m.remove(&c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_codes_use_the_safe_alphabet() {
        for _ in 0..200 {
            let c = new_user_code();
            assert_eq!(c.len(), 8);
            assert!(c.bytes().all(|b| USER_CODE_ALPHABET.contains(&b)), "{c}");
        }
    }

    #[test]
    fn typed_codes_normalize() {
        assert_eq!(normalize_user_code(" bcdf-ghjk\n"), "BCDFGHJK");
        assert_eq!(
            normalize_user_code("BCDFGHJKLMNP").len(),
            9,
            "never matches"
        );
    }

    #[test]
    fn device_labels_are_bounded_and_clean() {
        assert_eq!(
            clean_label(Some(" T\u{200b}V\u{202e}\n ".into()), 64).as_deref(),
            Some("TV")
        );
        assert_eq!(clean_label(Some("x".repeat(99)), 64).unwrap().len(), 64);
        assert_eq!(clean_label(Some(" \n ".into()), 64), None);
    }
}
