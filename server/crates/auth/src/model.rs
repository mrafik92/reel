use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    User,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
    pub fn parse(s: &str) -> Role {
        if s == "admin" {
            Role::Admin
        } else {
            Role::User
        }
    }
}

/// Per-user limits (`user_limits`). `None` = unlimited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub max_streams: Option<i64>,
    pub max_transcodes: Option<i64>,
    pub max_bitrate_kbps: Option<i64>,
    pub request_quota_movies: Option<i64>,
    pub request_quota_tv: Option<i64>,
    pub quota_window_days: i64,
    pub auto_approve: bool,
}

impl Limits {
    /// Conservative preset for accounts created from an invite without explicit limits.
    pub fn invite_default() -> Self {
        Self {
            max_streams: Some(2),
            max_transcodes: Some(1),
            max_bitrate_kbps: Some(8000),
            request_quota_movies: Some(3),
            request_quota_tv: Some(3),
            quota_window_days: 7,
            auto_approve: false,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_streams: None,
            max_transcodes: None,
            max_bitrate_kbps: None,
            request_quota_movies: None,
            request_quota_tv: None,
            quota_window_days: 7,
            auto_approve: false,
        }
    }
}

/// Per-user feature permissions. `Default` = nothing allowed; [`Permissions::all`] = everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Permissions {
    pub iptv_access: bool,
    pub vod_access: bool,
    pub can_request: bool,
    /// Play on TVs (DLNA). Admins always can.
    pub can_cast: bool,
}

impl Permissions {
    pub fn all() -> Self {
        Self {
            iptv_access: true,
            vod_access: true,
            can_request: true,
            can_cast: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    pub created_at: i64,
    pub limits: Limits,
    pub perms: Permissions,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
    pub fn can_iptv(&self) -> bool {
        self.is_admin() || self.perms.iptv_access
    }
    pub fn can_vod(&self) -> bool {
        self.is_admin() || self.perms.vod_access
    }
    pub fn can_request(&self) -> bool {
        self.is_admin() || self.perms.can_request
    }
    pub fn can_cast(&self) -> bool {
        self.is_admin() || self.perms.can_cast
    }
}

/// Device metadata supplied at login.
#[derive(Debug, Clone, Default)]
pub struct Device {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
    pub ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionInfo {
    pub id: i64,
    pub user_id: i64,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub client: Option<String>,
    pub ip: Option<String>,
    pub last_seen: i64,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub username: String,
    pub password: String,
    pub role: Role,
    pub limits: Limits,
    pub library_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UserPatch {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<Role>,
    pub disabled: Option<bool>,
    pub limits: Option<Limits>,
    pub library_ids: Option<Vec<i64>>,
    pub perms: Option<Permissions>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Invite {
    pub id: i64,
    pub code: String,
    pub created_by: i64,
    pub library_ids: Vec<i64>,
    pub limits: Limits,
    /// Permissions of the account the invite creates (default: none).
    pub perms: Permissions,
    pub expires_at: Option<i64>,
    pub used_by: Option<i64>,
    pub used_at: Option<i64>,
}

/// Public view of an invite (for the accept page).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InviteInfo {
    pub valid: bool,
    pub expires_at: Option<i64>,
    pub library_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    pub actor_id: Option<i64>,
    pub actor_name: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub detail: Option<Value>,
    pub ip: Option<String>,
}
