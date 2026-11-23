//! Second sign-in factor (#146): the login challenge, TOTP, recovery codes and passkeys.

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use hmac::{Hmac, Mac};
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder,
};

use super::*;
use crate::totp;

const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);
const REGISTRATION_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_CHALLENGE_ATTEMPTS: u8 = 5;
const MAX_PENDING: usize = 5_000;
const RECOVERY_CODES: usize = 10;
const MAX_PASSKEYS: i64 = 10;
/// No `0`/`o`/`1`/`l`/`i`: codes get copied by hand from paper.
const RECOVERY_ALPHABET: &[u8; 32] = b"23456789abcdefghjkmnpqrstuvwxyz-";

/// A correct password on an account with a second factor: what the client needs for step two.
#[derive(Debug, Clone)]
pub struct MfaChallenge {
    /// Opaque, single-purpose token; it can't be used as a session.
    pub token: String,
    /// `totp`, `passkey` and/or `recovery`.
    pub methods: Vec<&'static str>,
    pub expires_in_s: u64,
}

/// The second step of a login.
#[derive(Debug)]
pub enum MfaProof {
    Totp(String),
    Recovery(String),
    /// The browser's WebAuthn assertion, as JSON.
    Passkey(Value),
}

#[derive(Debug, Clone)]
pub struct PasskeyInfo {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MfaStatus {
    pub totp: bool,
    pub passkeys: Vec<PasskeyInfo>,
    pub recovery_codes_left: i64,
    /// The server can do WebAuthn (`public_url` is set and valid).
    pub passkeys_available: bool,
}

/// A TOTP enrollment waiting for its first code.
#[derive(Debug, Clone)]
pub struct TotpEnrollment {
    pub secret: String,
    pub otpauth_uri: String,
}

struct PendingLogin {
    user_id: i64,
    dev: Device,
    password_weak: bool,
    expires: Instant,
    attempts: u8,
    passkey: Option<PasskeyAuthentication>,
}

struct PendingRegistration {
    name: String,
    state: PasskeyRegistration,
    expires: Instant,
}

/// In-memory on purpose: a restart only means entering the password again.
#[derive(Default)]
pub(super) struct MfaState {
    logins: Mutex<HashMap<String, PendingLogin>>,
    registrations: Mutex<HashMap<i64, PendingRegistration>>,
    require_admin: AtomicBool,
    webauthn: Option<Webauthn>,
}

impl MfaState {
    pub(super) fn new(public_url: Option<&str>) -> Self {
        Self {
            webauthn: public_url.and_then(build_webauthn),
            ..Default::default()
        }
    }
}

fn build_webauthn(public_url: &str) -> Option<Webauthn> {
    let url = Url::parse(public_url.trim()).ok()?;
    let host = url.host_str()?.to_string();
    let built = WebauthnBuilder::new(&host, &url).and_then(|b| b.rp_name("Reel").build());
    match built {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!("passkeys are off: can't use public_url for WebAuthn: {e}");
            None
        }
    }
}

fn normalize_recovery(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn new_recovery_code() -> String {
    // 31 usable symbols: rejection sampling keeps them uniform
    let mut out = String::with_capacity(11);
    let mut produced = 0;
    while produced < 10 {
        let mut b = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut b);
        for x in b {
            let v = x & 31;
            if v == 31 || produced == 10 {
                continue;
            }
            if produced == 5 {
                out.push('-');
            }
            out.push(RECOVERY_ALPHABET[v as usize] as char);
            produced += 1;
        }
    }
    out
}

fn bad_input(m: &str) -> AuthError {
    AuthError::BadRequest(m.into())
}

fn clean_passkey_name(name: &str) -> AuthResult<String> {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    if n.is_empty() || n.chars().count() > 64 {
        return Err(bad_input("give the passkey a name of 1-64 characters"));
    }
    Ok(n)
}

impl AuthService {
    fn keyed_hash(&self, purpose: &str, value: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("any key length");
        mac.update(purpose.as_bytes());
        mac.update(b":");
        mac.update(value.as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Stable WebAuthn user handle that doesn't reveal the numeric user id.
    fn webauthn_user_id(&self, user_id: i64) -> Uuid {
        let h = self.keyed_hash("webauthn-user", &user_id.to_string());
        let mut b = [0u8; 16];
        for (i, slot) in b.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap_or(0);
        }
        uuid_from(b)
    }

    fn webauthn(&self) -> AuthResult<&Webauthn> {
        self.mfa.webauthn.as_ref().ok_or_else(|| {
            AuthError::Conflict("passkeys need REEL_PUBLIC_URL to be set on the server".into())
        })
    }

    /// Require a second factor before an admin gets admin powers. Set once at startup.
    pub fn set_require_admin_2fa(&self, on: bool) {
        self.mfa.require_admin.store(on, Ordering::Relaxed);
    }

    pub async fn has_second_factor(&self, user_id: i64) -> AuthResult<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id = ? AND confirmed_at IS NOT NULL) \
             OR EXISTS(SELECT 1 FROM user_passkeys WHERE user_id = ?)",
        )
        .bind(user_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?)
    }

    /// An admin who still has to add a second factor before getting admin powers.
    pub async fn must_enroll(&self, user: &User) -> AuthResult<bool> {
        Ok(user.has_admin_role()
            && self.mfa.require_admin.load(Ordering::Relaxed)
            && !self.has_second_factor(user.id).await?)
    }

    pub async fn mfa_status(&self, user_id: i64) -> AuthResult<MfaStatus> {
        let totp: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM user_totp WHERE user_id = ? AND confirmed_at IS NOT NULL)",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let recovery_codes_left = sqlx::query_scalar(
            "SELECT count(*) FROM user_recovery_codes WHERE user_id = ? AND used_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT id, name, created_at, last_used_at FROM user_passkeys WHERE user_id = ? ORDER BY id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(MfaStatus {
            totp,
            recovery_codes_left,
            passkeys_available: self.mfa.webauthn.is_some(),
            passkeys: rows
                .iter()
                .map(|r| PasskeyInfo {
                    id: r.get("id"),
                    name: r.get("name"),
                    created_at: r.get("created_at"),
                    last_used_at: r.get("last_used_at"),
                })
                .collect(),
        })
    }

    /// Ask for the password again before a change to the account's sign-in methods. Failures count
    /// against their own per-account lock, so a stolen session can't guess the password for free.
    async fn reauthenticate(&self, user: &User, password: &str) -> AuthResult<()> {
        let key = format!("reauth:{}", user.id);
        let max = self.opts.limiter.user_max_failures;
        self.limiter.reserve(&key, max).map_err(AuthError::Locked)?;
        let phc: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(&self.pool)
            .await?;
        let ok = match self.verify_pw(password, &phc).await {
            Ok(v) => v,
            Err(e) => {
                self.limiter.refund(&key, max);
                return Err(e);
            }
        };
        if ok {
            self.limiter.clear(&key);
            Ok(())
        } else {
            Err(AuthError::InvalidCredentials)
        }
    }

    // ───────────── login, step two ─────────────

    fn sweep_logins(map: &mut HashMap<String, PendingLogin>) {
        let now = Instant::now();
        map.retain(|_, p| p.expires > now);
    }

    pub(super) async fn issue_challenge(
        &self,
        user: &User,
        dev: &Device,
        password_weak: bool,
    ) -> AuthResult<MfaChallenge> {
        let status = self.mfa_status(user.id).await?;
        let mut methods = Vec::with_capacity(3);
        if status.totp {
            methods.push("totp");
        }
        if !status.passkeys.is_empty() && status.passkeys_available {
            methods.push("passkey");
        }
        if status.recovery_codes_left > 0 {
            methods.push("recovery");
        }
        let token = random_b64(32);
        {
            let mut m = self.mfa.logins.lock().unwrap();
            if m.len() >= MAX_PENDING {
                Self::sweep_logins(&mut m);
                if m.len() >= MAX_PENDING {
                    return Err(AuthError::Locked(Duration::from_secs(5)));
                }
            }
            m.insert(
                hash_token(&token),
                PendingLogin {
                    user_id: user.id,
                    dev: dev.clone(),
                    password_weak,
                    expires: Instant::now() + CHALLENGE_TTL,
                    attempts: 0,
                    passkey: None,
                },
            );
        }
        Ok(MfaChallenge {
            token,
            methods,
            expires_in_s: CHALLENGE_TTL.as_secs(),
        })
    }

    /// Forget half-finished logins and uncollected TV approvals, e.g. after a password change.
    pub(super) fn drop_pending_logins(&self, user_id: i64) {
        self.drop_device_approvals(user_id);
        self.mfa
            .logins
            .lock()
            .unwrap()
            .retain(|_, p| p.user_id != user_id);
    }

    /// Options for `navigator.credentials.get()` during step two.
    pub async fn challenge_passkey_options(&self, token: &str) -> AuthResult<Value> {
        let wa = self.webauthn()?;
        let key = hash_token(token);
        let user_id = {
            let m = self.mfa.logins.lock().unwrap();
            match m.get(&key) {
                Some(p) if p.expires > Instant::now() => p.user_id,
                _ => return Err(AuthError::Unauthorized),
            }
        };
        let keys = self.load_passkeys(user_id).await?;
        if keys.is_empty() {
            return Err(bad_input("this account has no passkey"));
        }
        let creds: Vec<Passkey> = keys.into_iter().map(|(_, k)| k).collect();
        let (options, state) = wa
            .start_passkey_authentication(&creds)
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        let mut m = self.mfa.logins.lock().unwrap();
        let p = m.get_mut(&key).ok_or(AuthError::Unauthorized)?;
        p.passkey = Some(state);
        serde_json::to_value(options).map_err(|e| AuthError::Internal(e.to_string()))
    }

    /// Finish a login. The challenge is taken out of the map while it's checked, so parallel guesses
    /// on one challenge can't race.
    pub async fn complete_mfa(
        &self,
        token: &str,
        proof: MfaProof,
        ip: Option<&str>,
    ) -> AuthResult<LoginOutcome> {
        let key = hash_token(token);
        let mut pending = {
            let mut m = self.mfa.logins.lock().unwrap();
            match m.remove(&key) {
                Some(p) if p.expires > Instant::now() => p,
                _ => return Err(AuthError::Unauthorized),
            }
        };
        let user = match self.get_user(pending.user_id).await? {
            Some(u) if !u.disabled => u,
            _ => return Err(AuthError::Unauthorized),
        };
        let bucket = ip_bucket(ip);
        let ip_key = format!("ip:{bucket}");
        let mfa_key = format!("mfa:{}", user.id);
        let (ip_max, user_max) = (
            self.opts.limiter.ip_max_failures,
            self.opts.limiter.user_max_failures,
        );
        let put_back = |p: PendingLogin| {
            self.mfa.logins.lock().unwrap().insert(key.clone(), p);
        };
        if let Err(wait) = self.limiter.reserve(&ip_key, ip_max) {
            put_back(pending);
            return Err(AuthError::Locked(wait));
        }
        // per account, not per address: whoever reaches this step already has the password
        if let Err(wait) = self.limiter.reserve(&mfa_key, user_max) {
            self.limiter.refund(&ip_key, ip_max);
            put_back(pending);
            self.audit(
                Some(user.id),
                Some(&user.username),
                "login_mfa_locked",
                None,
                json!({"retry_after_s": wait.as_secs()}),
                ip,
            )
            .await;
            return Err(AuthError::Locked(wait));
        }
        let (method, verified) = match proof {
            MfaProof::Totp(code) => ("totp", self.consume_totp(user.id, &code).await),
            MfaProof::Recovery(code) => ("recovery", self.consume_recovery(user.id, &code).await),
            MfaProof::Passkey(cred) => (
                "passkey",
                self.verify_passkey(user.id, pending.passkey.take(), cred)
                    .await,
            ),
        };
        let verified = match verified {
            Ok(v) => v,
            Err(e) => {
                self.limiter.refund(&ip_key, ip_max);
                self.limiter.refund(&mfa_key, user_max);
                put_back(pending);
                return Err(e);
            }
        };
        let lname = user.username.to_lowercase();
        if !verified {
            self.limiter.soft_fail(&format!("uname:{lname}"));
            self.audit_failure("login_mfa_failure", &user.username, ip)
                .await;
            pending.attempts += 1;
            if pending.attempts < MAX_CHALLENGE_ATTEMPTS {
                put_back(pending);
            }
            return Err(AuthError::InvalidCredentials);
        }
        self.limiter.refund(&ip_key, ip_max);
        self.limiter.clear(&mfa_key);
        self.limiter.clear(&format!("user:{lname}|{bucket}"));
        self.limiter.clear(&format!("uname:{lname}"));
        let (session, expires_at) = self.create_session(user.id, &pending.dev).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "login_mfa",
            None,
            json!({"method": method}),
            ip,
        )
        .await;
        Ok(LoginOutcome {
            token: session,
            user,
            expires_at,
            password_weak: pending.password_weak,
        })
    }

    async fn consume_totp(&self, user_id: i64, code: &str) -> AuthResult<bool> {
        let row: Option<(Vec<u8>, i64)> = sqlx::query_as(
            "SELECT secret, last_step FROM user_totp WHERE user_id = ? AND confirmed_at IS NOT NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((sealed, last_step)) = row else {
            return Ok(false);
        };
        let secret = totp::open(&self.key, user_id, &sealed)
            .ok_or_else(|| AuthError::Internal("can't decrypt the TOTP secret".into()))?;
        let Some(step) = totp::matching_step(&secret, code, now_unix(), last_step) else {
            return Ok(false);
        };
        // only one request can move the step forward: the same code can't be used twice
        let n =
            sqlx::query("UPDATE user_totp SET last_step = ? WHERE user_id = ? AND last_step < ?")
                .bind(step)
                .bind(user_id)
                .bind(step)
                .execute(&self.pool)
                .await?
                .rows_affected();
        Ok(n == 1)
    }

    async fn consume_recovery(&self, user_id: i64, code: &str) -> AuthResult<bool> {
        let code = normalize_recovery(code);
        if code.len() != 10 {
            return Ok(false);
        }
        let n = sqlx::query(
            "UPDATE user_recovery_codes SET used_at = ? \
             WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
        )
        .bind(now_unix())
        .bind(user_id)
        .bind(self.keyed_hash("recovery", &code))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(n == 1)
    }

    async fn load_passkeys(&self, user_id: i64) -> AuthResult<Vec<(i64, Passkey)>> {
        let rows: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, passkey FROM user_passkeys WHERE user_id = ? ORDER BY id")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, j)| serde_json::from_str(&j).ok().map(|k| (id, k)))
            .collect())
    }

    async fn verify_passkey(
        &self,
        user_id: i64,
        state: Option<PasskeyAuthentication>,
        credential: Value,
    ) -> AuthResult<bool> {
        let wa = self.webauthn()?;
        let Some(state) = state else {
            return Ok(false);
        };
        let Ok(cred) = serde_json::from_value::<PublicKeyCredential>(credential) else {
            return Ok(false);
        };
        let Ok(result) = wa.finish_passkey_authentication(&cred, &state) else {
            return Ok(false);
        };
        for (id, mut key) in self.load_passkeys(user_id).await? {
            if key.cred_id() != result.cred_id() {
                continue;
            }
            key.update_credential(&result);
            let json =
                serde_json::to_string(&key).map_err(|e| AuthError::Internal(e.to_string()))?;
            sqlx::query("UPDATE user_passkeys SET passkey = ?, last_used_at = ? WHERE id = ?")
                .bind(json)
                .bind(now_unix())
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(true);
        }
        Ok(false)
    }

    // ───────────── enrollment ─────────────

    async fn replace_recovery_codes(&self, user_id: i64) -> AuthResult<Vec<String>> {
        let codes: Vec<String> = (0..RECOVERY_CODES).map(|_| new_recovery_code()).collect();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for c in &codes {
            sqlx::query("INSERT INTO user_recovery_codes (user_id, code_hash) VALUES (?, ?)")
                .bind(user_id)
                .bind(self.keyed_hash("recovery", &normalize_recovery(c)))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(codes)
    }

    /// Recovery codes for an account getting its first factor; `None` if it already has some.
    async fn first_recovery_codes(&self, user_id: i64) -> AuthResult<Option<Vec<String>>> {
        if self.mfa_status(user_id).await?.recovery_codes_left > 0 {
            return Ok(None);
        }
        Ok(Some(self.replace_recovery_codes(user_id).await?))
    }

    pub async fn regenerate_recovery_codes(
        &self,
        user: &User,
        password: &str,
    ) -> AuthResult<Vec<String>> {
        self.reauthenticate(user, password).await?;
        if !self.has_second_factor(user.id).await? {
            return Err(bad_input("add a passkey or authenticator app first"));
        }
        let codes = self.replace_recovery_codes(user.id).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "recovery_codes_new",
            None,
            Value::Null,
            None,
        )
        .await;
        Ok(codes)
    }

    pub async fn totp_start(&self, user: &User, password: &str) -> AuthResult<TotpEnrollment> {
        self.reauthenticate(user, password).await?;
        if self.mfa_status(user.id).await?.totp {
            return Err(AuthError::Conflict(
                "an authenticator app is already set up; remove it first".into(),
            ));
        }
        let secret = totp::new_secret();
        let sealed = totp::seal(&self.key, user.id, &secret)
            .ok_or_else(|| AuthError::Internal("can't encrypt the TOTP secret".into()))?;
        sqlx::query(
            "INSERT INTO user_totp (user_id, secret) VALUES (?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET secret = excluded.secret, last_step = 0 \
             WHERE confirmed_at IS NULL",
        )
        .bind(user.id)
        .bind(sealed)
        .execute(&self.pool)
        .await?;
        Ok(TotpEnrollment {
            otpauth_uri: totp::otpauth_uri("Reel", &user.username, &secret),
            secret: totp::base32(&secret),
        })
    }

    /// Activate TOTP with its first code. Returns recovery codes if this is the first factor.
    pub async fn totp_confirm(&self, user: &User, code: &str) -> AuthResult<Option<Vec<String>>> {
        let key = format!("reauth:{}", user.id);
        let max = self.opts.limiter.user_max_failures;
        self.limiter.reserve(&key, max).map_err(AuthError::Locked)?;
        let sealed: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT secret FROM user_totp WHERE user_id = ? AND confirmed_at IS NULL",
        )
        .bind(user.id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(sealed) = sealed else {
            self.limiter.refund(&key, max);
            return Err(bad_input("start the authenticator setup first"));
        };
        let secret = totp::open(&self.key, user.id, &sealed)
            .ok_or_else(|| AuthError::Internal("can't decrypt the TOTP secret".into()))?;
        let Some(step) = totp::matching_step(&secret, code, now_unix(), 0) else {
            return Err(AuthError::InvalidCredentials);
        };
        self.limiter.clear(&key);
        // the code that confirmed enrollment can't also be used to sign in
        sqlx::query(
            "UPDATE user_totp SET confirmed_at = ?, last_step = ? \
             WHERE user_id = ? AND confirmed_at IS NULL",
        )
        .bind(now_unix())
        .bind(step)
        .bind(user.id)
        .execute(&self.pool)
        .await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "mfa_add",
            None,
            json!({"method": "totp"}),
            None,
        )
        .await;
        self.first_recovery_codes(user.id).await
    }

    /// An admin must keep one factor while the requirement is on.
    async fn guard_last_factor(&self, user: &User, remaining: bool) -> AuthResult<()> {
        if !remaining && user.has_admin_role() && self.mfa.require_admin.load(Ordering::Relaxed) {
            return Err(AuthError::Conflict(
                "admins must keep a second factor; add another one before removing this one".into(),
            ));
        }
        Ok(())
    }

    async fn clear_recovery_if_unprotected(&self, user_id: i64) -> AuthResult<()> {
        if !self.has_second_factor(user_id).await? {
            sqlx::query("DELETE FROM user_recovery_codes WHERE user_id = ?")
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    pub async fn totp_remove(&self, user: &User, password: &str) -> AuthResult<()> {
        self.reauthenticate(user, password).await?;
        let s = self.mfa_status(user.id).await?;
        if !s.totp {
            return Err(AuthError::NotFound);
        }
        self.guard_last_factor(user, !s.passkeys.is_empty()).await?;
        sqlx::query("DELETE FROM user_totp WHERE user_id = ?")
            .bind(user.id)
            .execute(&self.pool)
            .await?;
        self.clear_recovery_if_unprotected(user.id).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "mfa_remove",
            None,
            json!({"method": "totp"}),
            None,
        )
        .await;
        Ok(())
    }

    /// Options for `navigator.credentials.create()`.
    pub async fn passkey_start(
        &self,
        user: &User,
        password: &str,
        name: &str,
    ) -> AuthResult<Value> {
        let wa = self.webauthn()?;
        let name = clean_passkey_name(name)?;
        self.reauthenticate(user, password).await?;
        let existing = self.load_passkeys(user.id).await?;
        if existing.len() as i64 >= MAX_PASSKEYS {
            return Err(AuthError::Conflict(format!(
                "an account can have at most {MAX_PASSKEYS} passkeys"
            )));
        }
        let exclude: Vec<_> = existing.iter().map(|(_, k)| k.cred_id().clone()).collect();
        let (options, state) = wa
            .start_passkey_registration(
                self.webauthn_user_id(user.id),
                &user.username,
                &user.username,
                (!exclude.is_empty()).then_some(exclude),
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        {
            let mut m = self.mfa.registrations.lock().unwrap();
            let now = Instant::now();
            m.retain(|_, r| r.expires > now);
            if m.len() >= MAX_PENDING {
                return Err(AuthError::Locked(Duration::from_secs(5)));
            }
            m.insert(
                user.id,
                PendingRegistration {
                    name,
                    state,
                    expires: now + REGISTRATION_TTL,
                },
            );
        }
        serde_json::to_value(options).map_err(|e| AuthError::Internal(e.to_string()))
    }

    /// Store the new passkey. Returns recovery codes if this is the first factor.
    pub async fn passkey_finish(
        &self,
        user: &User,
        credential: Value,
    ) -> AuthResult<Option<Vec<String>>> {
        let wa = self.webauthn()?;
        let pending = self
            .mfa
            .registrations
            .lock()
            .unwrap()
            .remove(&user.id)
            .filter(|r| r.expires > Instant::now())
            .ok_or_else(|| bad_input("start adding the passkey again"))?;
        let cred: RegisterPublicKeyCredential = serde_json::from_value(credential)
            .map_err(|_| bad_input("the browser's passkey response wasn't valid"))?;
        let key = wa
            .finish_passkey_registration(&cred, &pending.state)
            .map_err(|e| {
                tracing::info!("passkey registration refused: {e}");
                bad_input("the passkey couldn't be verified")
            })?;
        let cred_id = URL_SAFE_NO_PAD.encode::<&[u8]>(key.cred_id().as_ref());
        let json = serde_json::to_string(&key).map_err(|e| AuthError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO user_passkeys (user_id, credential_id, name, passkey) VALUES (?, ?, ?, ?)",
        )
        .bind(user.id)
        .bind(cred_id)
        .bind(&pending.name)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            if is_unique(&e) {
                AuthError::Conflict("this passkey is already registered".into())
            } else {
                e.into()
            }
        })?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "mfa_add",
            None,
            json!({"method": "passkey"}),
            None,
        )
        .await;
        self.first_recovery_codes(user.id).await
    }

    pub async fn passkey_rename(&self, user_id: i64, id: i64, name: &str) -> AuthResult<()> {
        let name = clean_passkey_name(name)?;
        let n = sqlx::query("UPDATE user_passkeys SET name = ? WHERE id = ? AND user_id = ?")
            .bind(name)
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        (n > 0).then_some(()).ok_or(AuthError::NotFound)
    }

    pub async fn passkey_remove(&self, user: &User, password: &str, id: i64) -> AuthResult<()> {
        self.reauthenticate(user, password).await?;
        let s = self.mfa_status(user.id).await?;
        if !s.passkeys.iter().any(|p| p.id == id) {
            return Err(AuthError::NotFound);
        }
        self.guard_last_factor(user, s.totp || s.passkeys.len() > 1)
            .await?;
        sqlx::query("DELETE FROM user_passkeys WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .execute(&self.pool)
            .await?;
        self.clear_recovery_if_unprotected(user.id).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "mfa_remove",
            None,
            json!({"method": "passkey"}),
            None,
        )
        .await;
        Ok(())
    }

    /// Admin recovery for someone who lost their factors: removes them all and signs the user out.
    pub async fn reset_second_factor(&self, user_id: i64) -> AuthResult<()> {
        self.get_user(user_id).await?.ok_or(AuthError::NotFound)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for table in ["user_totp", "user_passkeys", "user_recovery_codes"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE user_id = ?"))
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.drop_pending_logins(user_id);
        self.revoke_all_sessions(user_id).await?;
        Ok(())
    }
}

fn uuid_from(b: [u8; 16]) -> Uuid {
    Uuid::from_bytes(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_codes_are_well_formed_and_normalize() {
        for _ in 0..200 {
            let c = new_recovery_code();
            assert_eq!(c.len(), 11, "{c}");
            assert_eq!(c.as_bytes()[5], b'-');
            assert_eq!(normalize_recovery(&c).len(), 10);
            assert!(!c.contains(['0', 'o', '1', 'l', 'i']), "{c}");
        }
        assert_eq!(normalize_recovery(" AB2C3-defgh\n"), "ab2c3defgh");
    }

    #[test]
    fn passkey_names_are_bounded() {
        assert_eq!(clean_passkey_name("  Mac  ").unwrap(), "Mac");
        assert!(clean_passkey_name("   ").is_err());
        assert!(clean_passkey_name(&"x".repeat(65)).is_err());
    }
}
