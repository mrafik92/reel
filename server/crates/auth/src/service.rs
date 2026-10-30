use std::{path::Path, sync::Arc, time::Duration};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    AuditEntry, AuthError, AuthResult, Device, Invite, InviteInfo, LimiterConfig, Limits, NewUser,
    RateLimiter, Role, SessionInfo, User, UserPatch,
    password::{self, HashParams},
    signing::{UrlSigner, now_unix},
};

/// Don't rewrite the session row more often than this.
const TOUCH_INTERVAL: i64 = 60;

#[derive(Debug, Clone)]
pub struct AuthOptions {
    /// Sliding session lifetime.
    pub session_ttl: Duration,
    pub hash: HashParams,
    pub limiter: LimiterConfig,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            session_ttl: Duration::from_secs(30 * 24 * 3600),
            hash: HashParams::default(),
            limiter: LimiterConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoginOutcome {
    /// Opaque bearer token; shown once, only its SHA-256 is stored.
    pub token: String,
    pub user: User,
    pub expires_at: i64,
}

#[derive(Clone)]
pub struct AuthService {
    pool: SqlitePool,
    key: Arc<[u8]>,
    limiter: Arc<RateLimiter>,
    opts: Arc<AuthOptions>,
    dummy: Arc<std::sync::OnceLock<String>>,
}

fn random_b64(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Load the HMAC signing key from `path` (hex), generating and persisting one (0600) if absent.
pub fn load_or_create_signing_key(path: &Path) -> Vec<u8> {
    if let Ok(s) = std::fs::read_to_string(path) {
        let hex = s.trim();
        let bytes: Option<Vec<u8>> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
            .collect();
        if let Some(b) = bytes.filter(|b| b.len() >= 32) {
            return b;
        }
    }
    let mut key = vec![0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    let write = || -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, &hex)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        tracing::warn!("cannot persist signing key at {}: {e}", path.display());
    }
    key
}

fn validate_username(u: &str) -> AuthResult<String> {
    let u = u.trim();
    let ok = (3..=32).contains(&u.chars().count())
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(u.to_string())
    } else {
        Err(AuthError::BadRequest(
            "username must be 3-32 characters: letters, digits, '.', '_' or '-'".into(),
        ))
    }
}

fn validate_password(p: &str) -> AuthResult<()> {
    if p.chars().count() < 8 {
        return Err(AuthError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }
    if p.len() > 256 {
        return Err(AuthError::BadRequest("password is too long".into()));
    }
    Ok(())
}

const USER_SELECT: &str = "SELECT u.id, u.username, u.role, u.disabled, u.created_at, \
    l.max_streams, l.max_transcodes, l.max_bitrate_kbps, l.request_quota_movies, \
    l.request_quota_tv, l.quota_window_days, l.auto_approve \
    FROM users u LEFT JOIN user_limits l ON l.user_id = u.id";

fn user_from_row(r: &SqliteRow) -> User {
    User {
        id: r.get("id"),
        username: r.get("username"),
        role: Role::parse(&r.get::<String, _>("role")),
        disabled: r.get::<i64, _>("disabled") != 0,
        created_at: r.get("created_at"),
        limits: Limits {
            max_streams: r.get("max_streams"),
            max_transcodes: r.get("max_transcodes"),
            max_bitrate_kbps: r.get("max_bitrate_kbps"),
            request_quota_movies: r.get("request_quota_movies"),
            request_quota_tv: r.get("request_quota_tv"),
            quota_window_days: r.get::<Option<i64>, _>("quota_window_days").unwrap_or(7),
            auto_approve: r.get::<Option<i64>, _>("auto_approve").unwrap_or(0) != 0,
        },
    }
}

fn session_from_row(r: &SqliteRow) -> SessionInfo {
    SessionInfo {
        id: r.get("id"),
        user_id: r.get("user_id"),
        device_id: r.get("device_id"),
        device_name: r.get("device_name"),
        client: r.get("client"),
        ip: r.get("ip"),
        last_seen: r.get("last_seen"),
        expires_at: r.get("expires_at"),
    }
}

fn is_unique(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

impl AuthService {
    pub fn new(pool: SqlitePool, opts: AuthOptions, signing_key: Vec<u8>) -> Self {
        Self {
            pool,
            key: signing_key.into(),
            limiter: Arc::new(RateLimiter::new(opts.limiter.clone())),
            opts: Arc::new(opts),
            dummy: Arc::default(),
        }
    }

    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    pub fn signing_key(&self) -> &[u8] {
        &self.key
    }

    pub fn signer(&self, user_id: i64) -> UrlSigner {
        UrlSigner::new(self.key.clone(), user_id)
    }

    /// Verify a `(scope, user, exp, sig)` capability and return the (enabled) user it names.
    pub async fn verify_signature(
        &self,
        scope: &str,
        user_id: i64,
        exp: i64,
        sig: &str,
    ) -> AuthResult<User> {
        if !crate::signing::verify(&self.key, scope, user_id, exp, sig, now_unix()) {
            return Err(AuthError::Unauthorized);
        }
        match self.get_user(user_id).await? {
            Some(u) if !u.disabled => Ok(u),
            _ => Err(AuthError::Unauthorized),
        }
    }

    async fn hash_pw(&self, password: &str) -> AuthResult<String> {
        let (p, params) = (password.to_string(), self.opts.hash);
        tokio::task::spawn_blocking(move || password::hash(&p, params))
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?
    }

    async fn verify_pw(&self, password: &str, phc: &str) -> bool {
        let (p, h) = (password.to_string(), phc.to_string());
        tokio::task::spawn_blocking(move || password::verify(&p, &h))
            .await
            .unwrap_or(false)
    }

    /// Hash verified for unknown usernames (same cost as real ones).
    async fn dummy_hash(&self) -> String {
        if let Some(h) = self.dummy.get() {
            return h.clone();
        }
        let h = self
            .hash_pw("reel-dummy-password")
            .await
            .unwrap_or_default();
        self.dummy.get_or_init(|| h).clone()
    }

    // ───────────── setup ─────────────

    /// True until an account with a password exists (the seeded admin has none).
    pub async fn needs_setup(&self) -> AuthResult<bool> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE password_hash != '!'")
            .fetch_one(&self.pool)
            .await?;
        Ok(n == 0)
    }

    /// Claim the seeded admin account. Fails with `Conflict` once any password account exists.
    pub async fn setup(&self, username: &str, password: &str) -> AuthResult<User> {
        let username = validate_username(username)?;
        validate_password(password)?;
        let hash = self.hash_pw(password).await?;
        let claimed = sqlx::query(
            "UPDATE users SET username = ?, password_hash = ?, role = 'admin', disabled = 0 \
             WHERE id = 1 AND password_hash = '!' \
             AND NOT EXISTS (SELECT 1 FROM users WHERE password_hash != '!')",
        )
        .bind(&username)
        .bind(&hash)
        .execute(&self.pool)
        .await?
        .rows_affected();
        let id = if claimed == 1 {
            1
        } else if self.needs_setup().await? {
            // No seeded row (custom DB): create the admin.
            sqlx::query("INSERT INTO users (username, password_hash, role) VALUES (?, ?, 'admin')")
                .bind(&username)
                .bind(&hash)
                .execute(&self.pool)
                .await?
                .last_insert_rowid()
        } else {
            return Err(AuthError::Conflict("setup was already completed".into()));
        };
        self.audit(Some(id), Some(&username), "setup", None, Value::Null, None)
            .await;
        self.get_user(id).await?.ok_or(AuthError::NotFound)
    }

    // ───────────── sessions ─────────────

    async fn create_session(&self, user_id: i64, dev: &Device) -> AuthResult<(String, i64)> {
        let token = random_b64(32);
        let now = now_unix();
        let expires = now + self.opts.session_ttl.as_secs() as i64;
        if let Some(d) = &dev.device_id {
            // One session per device: re-login replaces the previous token.
            sqlx::query("DELETE FROM sessions_auth WHERE user_id = ? AND device_id = ?")
                .bind(user_id)
                .bind(d)
                .execute(&self.pool)
                .await?;
        }
        sqlx::query(
            "INSERT INTO sessions_auth (user_id, token_hash, device_id, device_name, client, ip, \
             last_seen, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(hash_token(&token))
        .bind(&dev.device_id)
        .bind(&dev.device_name)
        .bind(&dev.client)
        .bind(&dev.ip)
        .bind(now)
        .bind(expires)
        .execute(&self.pool)
        .await?;
        Ok((token, expires))
    }

    pub async fn login(
        &self,
        username: &str,
        password: &str,
        dev: Device,
    ) -> AuthResult<LoginOutcome> {
        let ip_key = format!("ip:{}", dev.ip.as_deref().unwrap_or("unknown"));
        let user_key = format!("user:{}", username.trim().to_lowercase());
        for k in [&ip_key, &user_key] {
            if let Err(wait) = self.limiter.check(k) {
                self.audit(
                    None,
                    Some(username),
                    "login_locked",
                    None,
                    json!({"retry_after_s": wait.as_secs()}),
                    dev.ip.as_deref(),
                )
                .await;
                return Err(AuthError::Locked(wait));
            }
        }
        let row = sqlx::query(&format!("{USER_SELECT} WHERE u.username = ?"))
            .bind(username.trim())
            .fetch_optional(&self.pool)
            .await?;
        let (user, phc) = match &row {
            Some(r) => (
                Some(user_from_row(r)),
                sqlx::query_scalar::<_, String>("SELECT password_hash FROM users WHERE id = ?")
                    .bind(r.get::<i64, _>("id"))
                    .fetch_one(&self.pool)
                    .await?,
            ),
            None => (None, self.dummy_hash().await),
        };
        // Always verify (against a dummy hash for unknown users) to equalize timing.
        let ok = self.verify_pw(password, &phc).await;
        match user {
            Some(u) if ok && !u.disabled => {
                self.limiter.clear(&user_key);
                let (token, expires_at) = self.create_session(u.id, &dev).await?;
                Ok(LoginOutcome {
                    token,
                    user: u,
                    expires_at,
                })
            }
            _ => {
                self.limiter.fail_user(&user_key);
                self.limiter.fail_ip(&ip_key);
                self.audit(
                    None,
                    Some(username),
                    "login_failure",
                    None,
                    Value::Null,
                    dev.ip.as_deref(),
                )
                .await;
                Err(AuthError::InvalidCredentials)
            }
        }
    }

    /// Resolve a bearer token; extends the session (sliding expiry).
    pub async fn authenticate(&self, token: &str) -> AuthResult<(User, SessionInfo)> {
        let row = sqlx::query(
            "SELECT id, user_id, device_id, device_name, client, ip, last_seen, expires_at \
             FROM sessions_auth WHERE token_hash = ?",
        )
        .bind(hash_token(token))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AuthError::Unauthorized)?;
        let mut s = session_from_row(&row);
        let now = now_unix();
        if s.expires_at.is_some_and(|e| e <= now) {
            let _ = sqlx::query("DELETE FROM sessions_auth WHERE id = ?")
                .bind(s.id)
                .execute(&self.pool)
                .await;
            return Err(AuthError::Unauthorized);
        }
        let user = match self.get_user(s.user_id).await? {
            Some(u) if !u.disabled => u,
            _ => return Err(AuthError::Unauthorized),
        };
        if now - s.last_seen >= TOUCH_INTERVAL {
            let expires = now + self.opts.session_ttl.as_secs() as i64;
            sqlx::query("UPDATE sessions_auth SET last_seen = ?, expires_at = ? WHERE id = ?")
                .bind(now)
                .bind(expires)
                .bind(s.id)
                .execute(&self.pool)
                .await?;
            s.last_seen = now;
            s.expires_at = Some(expires);
        }
        Ok((user, s))
    }

    pub async fn logout(&self, session_id: i64) -> AuthResult<()> {
        sqlx::query("DELETE FROM sessions_auth WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_sessions(&self, user_id: i64) -> AuthResult<Vec<SessionInfo>> {
        let rows = sqlx::query(
            "SELECT id, user_id, device_id, device_name, client, ip, last_seen, expires_at \
             FROM sessions_auth WHERE user_id = ? ORDER BY last_seen DESC, id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(session_from_row).collect())
    }

    /// Revoke one of `user_id`'s sessions.
    pub async fn revoke_session(&self, user_id: i64, session_id: i64) -> AuthResult<()> {
        let n = sqlx::query("DELETE FROM sessions_auth WHERE id = ? AND user_id = ?")
            .bind(session_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        (n > 0).then_some(()).ok_or(AuthError::NotFound)
    }

    /// Revoke every session of a user; returns how many.
    pub async fn revoke_all_sessions(&self, user_id: i64) -> AuthResult<u64> {
        Ok(sqlx::query("DELETE FROM sessions_auth WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    pub async fn change_password(
        &self,
        user_id: i64,
        current: &str,
        new: &str,
        keep_session: Option<i64>,
    ) -> AuthResult<()> {
        validate_password(new)?;
        let phc: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(AuthError::NotFound)?;
        if !self.verify_pw(current, &phc).await {
            return Err(AuthError::InvalidCredentials);
        }
        let hash = self.hash_pw(new).await?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(hash)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM sessions_auth WHERE user_id = ? AND id != ?")
            .bind(user_id)
            .bind(keep_session.unwrap_or(-1))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ───────────── users ─────────────

    pub async fn get_user(&self, id: i64) -> AuthResult<Option<User>> {
        Ok(sqlx::query(&format!("{USER_SELECT} WHERE u.id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| user_from_row(&r)))
    }

    /// Accounts that can sign in (excludes the unclaimed seeded admin).
    pub async fn list_users(&self) -> AuthResult<Vec<User>> {
        let rows = sqlx::query(&format!(
            "{USER_SELECT} WHERE u.password_hash != '!' ORDER BY u.id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(user_from_row).collect())
    }

    /// Library ids the user was granted explicitly (irrelevant for admins).
    pub async fn library_ids(&self, user_id: i64) -> AuthResult<Vec<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT library_id FROM library_access WHERE user_id = ? ORDER BY library_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Libraries visible to `user`: `None` = all (admins).
    pub async fn visible_libraries(&self, user: &User) -> AuthResult<Option<Vec<i64>>> {
        if user.is_admin() {
            return Ok(None);
        }
        Ok(Some(self.library_ids(user.id).await?))
    }

    async fn upsert_limits(
        tx: &mut sqlx::SqliteConnection,
        user_id: i64,
        l: &Limits,
    ) -> AuthResult<()> {
        sqlx::query(
            "INSERT INTO user_limits (user_id, max_streams, max_transcodes, max_bitrate_kbps, \
             request_quota_movies, request_quota_tv, quota_window_days, auto_approve) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET max_streams = excluded.max_streams, \
             max_transcodes = excluded.max_transcodes, max_bitrate_kbps = excluded.max_bitrate_kbps, \
             request_quota_movies = excluded.request_quota_movies, \
             request_quota_tv = excluded.request_quota_tv, \
             quota_window_days = excluded.quota_window_days, auto_approve = excluded.auto_approve",
        )
        .bind(user_id)
        .bind(l.max_streams)
        .bind(l.max_transcodes)
        .bind(l.max_bitrate_kbps)
        .bind(l.request_quota_movies)
        .bind(l.request_quota_tv)
        .bind(l.quota_window_days.max(1))
        .bind(i64::from(l.auto_approve))
        .execute(&mut *tx)
        .await?;
        Ok(())
    }

    async fn set_access(
        tx: &mut sqlx::SqliteConnection,
        user_id: i64,
        libs: &[i64],
    ) -> AuthResult<()> {
        sqlx::query("DELETE FROM library_access WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for l in libs {
            let r = sqlx::query(
                "INSERT OR IGNORE INTO library_access (user_id, library_id) VALUES (?, ?)",
            )
            .bind(user_id)
            .bind(l)
            .execute(&mut *tx)
            .await;
            if matches!(&r, Err(sqlx::Error::Database(d)) if d.is_foreign_key_violation()) {
                return Err(AuthError::BadRequest(format!("library {l} does not exist")));
            }
            r?;
        }
        Ok(())
    }

    pub async fn create_user(&self, nu: NewUser) -> AuthResult<User> {
        let username = validate_username(&nu.username)?;
        validate_password(&nu.password)?;
        let hash = self.hash_pw(&nu.password).await?;
        let mut tx = self.pool.begin().await?;
        let id = self
            .insert_user(
                &mut tx,
                &username,
                &hash,
                nu.role,
                &nu.limits,
                &nu.library_ids,
            )
            .await?;
        tx.commit().await?;
        self.get_user(id).await?.ok_or(AuthError::NotFound)
    }

    async fn insert_user(
        &self,
        tx: &mut sqlx::SqliteConnection,
        username: &str,
        hash: &str,
        role: Role,
        limits: &Limits,
        libs: &[i64],
    ) -> AuthResult<i64> {
        let id = sqlx::query("INSERT INTO users (username, password_hash, role) VALUES (?, ?, ?)")
            .bind(username)
            .bind(hash)
            .bind(role.as_str())
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                if is_unique(&e) {
                    AuthError::Conflict("username is already taken".into())
                } else {
                    e.into()
                }
            })?
            .last_insert_rowid();
        Self::upsert_limits(tx, id, limits).await?;
        Self::set_access(tx, id, libs).await?;
        Ok(id)
    }

    async fn other_enabled_admins(&self, except: i64) -> AuthResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE role = 'admin' AND disabled = 0 \
             AND password_hash != '!' AND id != ?",
        )
        .bind(except)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn update_user(&self, id: i64, p: UserPatch) -> AuthResult<User> {
        let cur = self.get_user(id).await?.ok_or(AuthError::NotFound)?;
        let loses_admin = cur.is_admin()
            && !cur.disabled
            && (p.role == Some(Role::User) || p.disabled == Some(true));
        if loses_admin && self.other_enabled_admins(id).await? == 0 {
            return Err(AuthError::Conflict(
                "cannot demote or disable the last admin".into(),
            ));
        }
        let username = match &p.username {
            Some(u) => validate_username(u)?,
            None => cur.username.clone(),
        };
        let hash = match &p.password {
            Some(pw) => {
                validate_password(pw)?;
                Some(self.hash_pw(pw).await?)
            }
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE users SET username = ?, role = ?, disabled = ? WHERE id = ?")
            .bind(&username)
            .bind(p.role.unwrap_or(cur.role).as_str())
            .bind(i64::from(p.disabled.unwrap_or(cur.disabled)))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                if is_unique(&e) {
                    AuthError::Conflict("username is already taken".into())
                } else {
                    e.into()
                }
            })?;
        if let Some(h) = &hash {
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(h)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        if let Some(l) = &p.limits {
            Self::upsert_limits(&mut tx, id, l).await?;
        }
        if let Some(libs) = &p.library_ids {
            Self::set_access(&mut tx, id, libs).await?;
        }
        if hash.is_some() || p.disabled == Some(true) {
            sqlx::query("DELETE FROM sessions_auth WHERE user_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.get_user(id).await?.ok_or(AuthError::NotFound)
    }

    pub async fn delete_user(&self, id: i64) -> AuthResult<()> {
        let cur = self.get_user(id).await?.ok_or(AuthError::NotFound)?;
        if cur.is_admin() && !cur.disabled && self.other_enabled_admins(id).await? == 0 {
            return Err(AuthError::Conflict("cannot delete the last admin".into()));
        }
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ───────────── invites ─────────────

    fn invite_from_row(r: &SqliteRow) -> Invite {
        Invite {
            id: r.get("id"),
            code: r.get("code"),
            created_by: r.get("created_by"),
            library_ids: serde_json::from_str(&r.get::<String, _>("library_ids"))
                .unwrap_or_default(),
            limits: serde_json::from_str(&r.get::<String, _>("limits_json")).unwrap_or_default(),
            expires_at: r.get("expires_at"),
            used_by: r.get("used_by"),
            used_at: r.get("used_at"),
        }
    }

    pub async fn create_invite(
        &self,
        created_by: i64,
        library_ids: Vec<i64>,
        limits: Limits,
        ttl: Option<Duration>,
    ) -> AuthResult<Invite> {
        if !library_ids.is_empty() {
            let ids = serde_json::to_string(&library_ids).unwrap_or_default();
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM libraries WHERE id IN (SELECT value FROM json_each(?))",
            )
            .bind(ids)
            .fetch_one(&self.pool)
            .await?;
            if n as usize
                != library_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            {
                return Err(AuthError::BadRequest("unknown library id".into()));
            }
        }
        let code = random_b64(18);
        let expires = ttl.map(|t| now_unix() + t.as_secs() as i64);
        let id = sqlx::query(
            "INSERT INTO invites (code, created_by, library_ids, limits_json, expires_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&code)
        .bind(created_by)
        .bind(serde_json::to_string(&library_ids).unwrap_or_default())
        .bind(serde_json::to_string(&limits).unwrap_or_default())
        .bind(expires)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        self.get_invite(id).await?.ok_or(AuthError::NotFound)
    }

    async fn get_invite(&self, id: i64) -> AuthResult<Option<Invite>> {
        Ok(sqlx::query("SELECT * FROM invites WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| Self::invite_from_row(&r)))
    }

    pub async fn list_invites(&self) -> AuthResult<Vec<Invite>> {
        let rows = sqlx::query("SELECT * FROM invites ORDER BY id DESC")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(Self::invite_from_row).collect())
    }

    pub async fn delete_invite(&self, id: i64) -> AuthResult<()> {
        let n = sqlx::query("DELETE FROM invites WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        (n > 0).then_some(()).ok_or(AuthError::NotFound)
    }

    /// Public check used by the invite landing page.
    pub async fn invite_info(&self, code: &str) -> AuthResult<InviteInfo> {
        let row = sqlx::query("SELECT * FROM invites WHERE code = ?")
            .bind(code)
            .fetch_optional(&self.pool)
            .await?;
        let Some(inv) = row.as_ref().map(Self::invite_from_row) else {
            return Ok(InviteInfo {
                valid: false,
                expires_at: None,
                library_names: vec![],
            });
        };
        let valid = inv.used_by.is_none() && inv.expires_at.is_none_or(|e| e > now_unix());
        let names = if valid {
            let ids = serde_json::to_string(&inv.library_ids).unwrap_or_default();
            sqlx::query_scalar(
                "SELECT name FROM libraries WHERE id IN (SELECT value FROM json_each(?)) ORDER BY name",
            )
            .bind(ids)
            .fetch_all(&self.pool)
            .await?
        } else {
            vec![]
        };
        Ok(InviteInfo {
            valid,
            expires_at: inv.expires_at.filter(|_| valid),
            library_names: names,
        })
    }

    /// Consume an invite: create the account with the preset access + limits and sign it in.
    pub async fn accept_invite(
        &self,
        code: &str,
        username: &str,
        password: &str,
        dev: Device,
    ) -> AuthResult<LoginOutcome> {
        let ip_key = format!("ip:{}", dev.ip.as_deref().unwrap_or("unknown"));
        if let Err(wait) = self.limiter.check(&ip_key) {
            return Err(AuthError::Locked(wait));
        }
        let username = validate_username(username)?;
        validate_password(password)?;
        let row = sqlx::query("SELECT * FROM invites WHERE code = ?")
            .bind(code)
            .fetch_optional(&self.pool)
            .await?;
        let inv = match row.as_ref().map(Self::invite_from_row) {
            Some(i) if i.used_by.is_none() && i.expires_at.is_none_or(|e| e > now_unix()) => i,
            _ => {
                self.limiter.fail_ip(&ip_key);
                return Err(AuthError::NotFound);
            }
        };
        let hash = self.hash_pw(password).await?;
        let mut tx = self.pool.begin().await?;
        let uid = self
            .insert_user(
                &mut tx,
                &username,
                &hash,
                Role::User,
                &inv.limits,
                &inv.library_ids,
            )
            .await?;
        // Single use: only one concurrent accept can flip `used_by`.
        let claimed = sqlx::query(
            "UPDATE invites SET used_by = ?, used_at = ? WHERE id = ? AND used_by IS NULL \
             AND (expires_at IS NULL OR expires_at > ?)",
        )
        .bind(uid)
        .bind(now_unix())
        .bind(inv.id)
        .bind(now_unix())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if claimed == 0 {
            tx.rollback().await?;
            return Err(AuthError::NotFound);
        }
        tx.commit().await?;
        self.audit(
            Some(uid),
            Some(&username),
            "invite_accept",
            Some(&inv.id.to_string()),
            Value::Null,
            dev.ip.as_deref(),
        )
        .await;
        let user = self.get_user(uid).await?.ok_or(AuthError::NotFound)?;
        let (token, expires_at) = self.create_session(uid, &dev).await?;
        Ok(LoginOutcome {
            token,
            user,
            expires_at,
        })
    }

    // ───────────── audit ─────────────

    /// Append an audit entry; failures are logged, never propagated.
    pub async fn audit(
        &self,
        actor_id: Option<i64>,
        actor_name: Option<&str>,
        action: &str,
        target: Option<&str>,
        detail: Value,
        ip: Option<&str>,
    ) {
        let detail = (!detail.is_null()).then(|| detail.to_string());
        let r = sqlx::query(
            "INSERT INTO audit_log (ts, actor_id, actor_name, action, target, detail, ip) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now_unix())
        .bind(actor_id)
        .bind(actor_name)
        .bind(action)
        .bind(target)
        .bind(detail)
        .bind(ip)
        .execute(&self.pool)
        .await;
        if let Err(e) = r {
            tracing::warn!("audit log write failed: {e}");
        }
    }

    /// Newest first; `before` = id cursor.
    pub async fn list_audit(&self, limit: i64, before: Option<i64>) -> AuthResult<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT id, ts, actor_id, actor_name, action, target, detail, ip FROM audit_log \
             WHERE (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?",
        )
        .bind(before)
        .bind(before)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| AuditEntry {
                id: r.get("id"),
                ts: r.get("ts"),
                actor_id: r.get("actor_id"),
                actor_name: r.get("actor_name"),
                action: r.get("action"),
                target: r.get("target"),
                detail: r
                    .get::<Option<String>, _>("detail")
                    .and_then(|d| serde_json::from_str(&d).ok()),
                ip: r.get("ip"),
            })
            .collect())
    }
}
