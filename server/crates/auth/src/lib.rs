//! Auth: Argon2id passwords, opaque hashed session tokens, invites, permissions, rate limiting
//! a second factor (TOTP, passkeys, recovery codes) and signed short-lived media URLs (SPEC §8).

mod error;
mod model;
pub mod password;
pub mod ratelimit;
mod service;
pub mod signing;
pub mod totp;

pub use error::{AuthError, AuthResult};
pub use model::{
    AuditEntry, Device, Invite, InviteInfo, Limits, NewUser, Permissions, Role, SessionInfo, User,
    UserPatch,
};
pub use ratelimit::{LimiterConfig, RateLimiter};
pub use service::{
    AuthOptions, AuthService, DevicePairing, LoginOutcome, MfaChallenge, MfaProof, MfaStatus,
    PairingInfo, PasskeyInfo, TotpEnrollment, ip_bucket, load_or_create_signing_key,
};
pub use signing::UrlSigner;
