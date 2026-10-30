//! Argon2id password hashing (PHC strings; parameters travel with the hash).

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::{AuthError, AuthResult};

/// Hasher configuration. `None` = Argon2 defaults (19 MiB, t=2, p=1).
#[derive(Debug, Clone, Copy, Default)]
pub struct HashParams {
    pub memory_kib: Option<u32>,
    pub iterations: Option<u32>,
}

fn argon(p: HashParams) -> AuthResult<Argon2<'static>> {
    match (p.memory_kib, p.iterations) {
        (None, None) => Ok(Argon2::default()),
        (m, t) => {
            let d = Params::default();
            let params = Params::new(
                m.unwrap_or(d.m_cost()),
                t.unwrap_or(d.t_cost()),
                d.p_cost(),
                None,
            )
            .map_err(|e| AuthError::Internal(e.to_string()))?;
            Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
        }
    }
}

/// Hash a password into a PHC string.
pub fn hash(password: &str, p: HashParams) -> AuthResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    argon(p)?
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AuthError::Internal(e.to_string()))
}

/// Verify against a PHC string. Unparsable hashes (e.g. the `!` placeholder) never match.
pub fn verify(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(h) => Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_params() {
        let p = HashParams {
            memory_kib: Some(512),
            iterations: Some(1),
        };
        let h = hash("hunter2hunter2", p).unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("hunter2hunter2", &h));
        assert!(!verify("wrong", &h));
        assert!(!verify("x", "!"));
        assert_ne!(h, hash("hunter2hunter2", p).unwrap(), "salted");
    }
}
