//! Provider configuration and linked identities for OpenID Connect sign-in (#149).

use super::*;
use crate::totp;

const SECRET_DOMAIN: &[u8] = b"reel-oidc-client-secret-v1";

/// A configured provider. The client secret stays inside this service.
#[derive(Debug, Clone)]
pub struct OidcProvider {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub issuer: String,
    pub discovery_url: String,
    pub client_id: String,
    pub secret_set: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct NewOidcProvider {
    pub slug: String,
    pub name: String,
    pub issuer: String,
    pub discovery_url: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Default)]
pub struct OidcProviderPatch {
    pub name: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub enabled: Option<bool>,
}

/// A verified login at a provider: `(issuer, subject)` is the identity.
#[derive(Debug, Clone)]
pub struct VerifiedIdentity {
    pub provider_id: i64,
    pub issuer: String,
    pub subject: String,
    /// Display only.
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LinkedIdentity {
    pub id: i64,
    pub provider: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

fn provider_from_row(r: &SqliteRow) -> OidcProvider {
    OidcProvider {
        id: r.get("id"),
        slug: r.get("slug"),
        name: r.get("name"),
        issuer: r.get("issuer"),
        discovery_url: r.get("discovery_url"),
        client_id: r.get("client_id"),
        secret_set: r.get::<i64, _>("secret_set") != 0,
        enabled: r.get::<i64, _>("enabled") != 0,
    }
}

const PROVIDER_SELECT: &str = "SELECT id, slug, name, issuer, discovery_url, client_id, enabled, \
    client_secret IS NOT NULL AS secret_set FROM oidc_providers";

fn bounded(field: &str, v: &str, max: usize) -> AuthResult<String> {
    let v = v.trim();
    if v.is_empty() || v.chars().count() > max || v.chars().any(char::is_control) {
        return Err(AuthError::BadRequest(format!("{field} isn't valid")));
    }
    Ok(v.to_string())
}

impl AuthService {
    fn seal_client_secret(&self, id: i64, secret: &str) -> AuthResult<Vec<u8>> {
        totp::seal_for(SECRET_DOMAIN, &self.key, id, secret.as_bytes())
            .ok_or_else(|| AuthError::Internal("can't encrypt the client secret".into()))
    }

    pub async fn list_oidc_providers(&self) -> AuthResult<Vec<OidcProvider>> {
        let rows = sqlx::query(&format!("{PROVIDER_SELECT} ORDER BY name, id"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(provider_from_row).collect())
    }

    /// An enabled provider, by slug.
    pub async fn oidc_provider(&self, slug: &str) -> AuthResult<OidcProvider> {
        sqlx::query(&format!("{PROVIDER_SELECT} WHERE slug = ? AND enabled = 1"))
            .bind(slug)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| provider_from_row(&r))
            .ok_or(AuthError::NotFound)
    }

    pub async fn oidc_provider_by_id(&self, id: i64) -> AuthResult<OidcProvider> {
        sqlx::query(&format!("{PROVIDER_SELECT} WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|r| provider_from_row(&r))
            .ok_or(AuthError::NotFound)
    }

    pub async fn oidc_client_secret(&self, id: i64) -> AuthResult<String> {
        let sealed: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT client_secret FROM oidc_providers WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .flatten();
        let sealed = sealed.ok_or(AuthError::NotFound)?;
        let plain = totp::open_for(SECRET_DOMAIN, &self.key, id, &sealed)
            .ok_or_else(|| AuthError::Internal("can't decrypt the client secret".into()))?;
        String::from_utf8(plain).map_err(|e| AuthError::Internal(e.to_string()))
    }

    pub async fn create_oidc_provider(&self, p: NewOidcProvider) -> AuthResult<OidcProvider> {
        let name = bounded("name", &p.name, 64)?;
        let client_id = bounded("client id", &p.client_id, 512)?;
        let secret = bounded("client secret", &p.client_secret, 1024)?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let id = sqlx::query(
            "INSERT INTO oidc_providers (slug, name, issuer, discovery_url, client_id) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&p.slug)
        .bind(&name)
        .bind(&p.issuer)
        .bind(&p.discovery_url)
        .bind(&client_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if is_unique(&e) {
                AuthError::Conflict("that provider is already configured".into())
            } else {
                e.into()
            }
        })?
        .last_insert_rowid();
        // the id is part of the encryption, so the secret is written once it's known
        sqlx::query("UPDATE oidc_providers SET client_secret = ? WHERE id = ?")
            .bind(self.seal_client_secret(id, &secret)?)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.oidc_provider_by_id(id).await
    }

    pub async fn update_oidc_provider(
        &self,
        id: i64,
        p: OidcProviderPatch,
    ) -> AuthResult<OidcProvider> {
        let cur = self.oidc_provider_by_id(id).await?;
        let name = match &p.name {
            Some(n) => bounded("name", n, 64)?,
            None => cur.name,
        };
        let client_id = match &p.client_id {
            Some(c) => bounded("client id", c, 512)?,
            None => cur.client_id,
        };
        sqlx::query("UPDATE oidc_providers SET name = ?, client_id = ?, enabled = ? WHERE id = ?")
            .bind(name)
            .bind(client_id)
            .bind(i64::from(p.enabled.unwrap_or(cur.enabled)))
            .bind(id)
            .execute(&self.pool)
            .await?;
        if let Some(s) = p.client_secret.as_deref().filter(|s| !s.trim().is_empty()) {
            let s = bounded("client secret", s, 1024)?;
            sqlx::query("UPDATE oidc_providers SET client_secret = ? WHERE id = ?")
                .bind(self.seal_client_secret(id, &s)?)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        self.oidc_provider_by_id(id).await
    }

    /// Remove a provider and every identity linked through it.
    pub async fn delete_oidc_provider(&self, id: i64) -> AuthResult<()> {
        let n = sqlx::query("DELETE FROM oidc_providers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        (n > 0).then_some(()).ok_or(AuthError::NotFound)
    }

    /// The account linked to `(issuer, subject)`. An email address is never consulted.
    pub async fn identity_user(&self, id: &VerifiedIdentity) -> AuthResult<Option<i64>> {
        let row: Option<(i64, i64)> = sqlx::query_as(
            "SELECT i.id, i.user_id FROM user_identities i \
             JOIN oidc_providers p ON p.id = i.provider_id \
             WHERE i.issuer = ? AND i.subject = ? AND p.enabled = 1",
        )
        .bind(&id.issuer)
        .bind(&id.subject)
        .fetch_optional(&self.pool)
        .await?;
        let Some((row_id, user_id)) = row else {
            return Ok(None);
        };
        sqlx::query("UPDATE user_identities SET last_used_at = ?, label = ? WHERE id = ?")
            .bind(now_unix())
            .bind(&id.label)
            .bind(row_id)
            .execute(&self.pool)
            .await?;
        Ok(Some(user_id))
    }

    pub(super) async fn insert_identity(
        conn: &mut sqlx::SqliteConnection,
        user_id: i64,
        id: &VerifiedIdentity,
    ) -> AuthResult<()> {
        sqlx::query(
            "INSERT INTO user_identities (user_id, provider_id, issuer, subject, label) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(id.provider_id)
        .bind(&id.issuer)
        .bind(&id.subject)
        .bind(&id.label)
        .execute(conn)
        .await
        .map_err(|e| {
            if is_unique(&e) {
                AuthError::Conflict("that sign-in is already linked to a Reel account".into())
            } else {
                e.into()
            }
        })?;
        Ok(())
    }

    pub async fn link_identity(&self, user: &User, id: &VerifiedIdentity) -> AuthResult<()> {
        let mut conn = self.pool.acquire().await?;
        Self::insert_identity(&mut conn, user.id, id).await?;
        drop(conn);
        self.audit(
            Some(user.id),
            Some(&user.username),
            "identity_link",
            Some(&id.provider_id.to_string()),
            Value::Null,
            None,
        )
        .await;
        Ok(())
    }

    pub async fn list_identities(&self, user_id: i64) -> AuthResult<Vec<LinkedIdentity>> {
        let rows = sqlx::query(
            "SELECT i.id, p.name AS provider, i.label, i.created_at, i.last_used_at \
             FROM user_identities i JOIN oidc_providers p ON p.id = i.provider_id \
             WHERE i.user_id = ? ORDER BY i.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| LinkedIdentity {
                id: r.get("id"),
                provider: r.get("provider"),
                label: r.get("label"),
                created_at: r.get("created_at"),
                last_used_at: r.get("last_used_at"),
            })
            .collect())
    }

    pub async fn unlink_identity(&self, user: &User, password: &str, id: i64) -> AuthResult<()> {
        self.confirm_password(user, password).await?;
        let n = sqlx::query("DELETE FROM user_identities WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user.id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(AuthError::NotFound);
        }
        self.audit(
            Some(user.id),
            Some(&user.username),
            "identity_unlink",
            Some(&id.to_string()),
            Value::Null,
            None,
        )
        .await;
        Ok(())
    }

    /// Sign in a user whose identity a provider vouched for. This replaces the password step only:
    /// an account with a second factor still has to pass it.
    pub async fn external_login(&self, user_id: i64, dev: &Device) -> AuthResult<LoginOutcome> {
        let user = match self.get_user(user_id).await? {
            Some(u) if !u.disabled => u,
            _ => return Err(AuthError::Unauthorized),
        };
        if self.has_second_factor(user.id).await? {
            let c = self.issue_challenge(&user, dev, false).await?;
            return Err(AuthError::SecondFactorRequired(c));
        }
        let (token, expires_at) = self.create_session(user.id, dev).await?;
        self.audit(
            Some(user.id),
            Some(&user.username),
            "login_oidc",
            None,
            Value::Null,
            dev.ip.as_deref(),
        )
        .await;
        Ok(LoginOutcome {
            token,
            user,
            expires_at,
            password_weak: false,
        })
    }
}
