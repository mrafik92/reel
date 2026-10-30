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

#[derive(Debug, Clone, PartialEq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    pub created_at: i64,
    pub limits: Limits,
}

impl User {
    pub fn is_admin(&self) -> bool {
        self.role == Role::Admin
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
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Invite {
    pub id: i64,
    pub code: String,
    pub created_by: i64,
    pub library_ids: Vec<i64>,
    pub limits: Limits,
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
