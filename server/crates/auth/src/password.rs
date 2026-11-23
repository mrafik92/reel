//! Argon2id password hashing (PHC strings; parameters travel with the hash) and the rule for new
//! passwords.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::{AuthError, AuthResult};

/// Shortest password accepted for a new or changed password (#148). Existing ones keep working.
pub const MIN_PASSWORD_CHARS: usize = 12;
const MAX_PASSWORD_BYTES: usize = 256;

/// The SecLists top-100k list (MIT, see `common_passwords.LICENSE`): lowercase, 6+ characters, sorted
/// bytewise. It's compiled in, so checking a password never contacts a third party.
const COMMON_PASSWORDS: &str = include_str!("common_passwords.txt");

/// Binary search over the embedded list, without building an in-memory set.
fn is_common(word: &str) -> bool {
    let list = COMMON_PASSWORDS.as_bytes();
    let (mut lo, mut hi) = (0, list.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = list[..mid]
            .iter()
            .rposition(|c| *c == b'\n')
            .map_or(0, |i| i + 1);
        let end = list[mid..]
            .iter()
            .position(|c| *c == b'\n')
            .map_or(list.len(), |i| mid + i);
        match list[start..end].cmp(word.as_bytes()) {
            std::cmp::Ordering::Equal => return true,
            std::cmp::Ordering::Less => lo = end + 1,
            std::cmp::Ordering::Greater => hi = start,
        }
    }
    false
}

/// `abcdefabcdef`: the whole password is one shorter unit repeated.
fn is_repeated_unit(chars: &[char]) -> bool {
    (1..=chars.len() / 2).any(|n| chars.len() % n == 0 && chars.chunks(n).all(|c| c == &chars[..n]))
}

/// The rule for a new or changed password. Login never applies it, so existing passwords still work.
pub fn check_policy(password: &str, username: Option<&str>) -> AuthResult<()> {
    let refuse = |m: String| Err(AuthError::BadRequest(m));
    let chars: Vec<char> = password.chars().collect();
    if chars.len() < MIN_PASSWORD_CHARS {
        return refuse(format!(
            "password must be at least {MIN_PASSWORD_CHARS} characters"
        ));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return refuse("password is too long".into());
    }
    let lower = password.to_lowercase();
    // `password1234!` is the usual way to pad a common word up to a length rule
    let base = lower.trim_matches(|c: char| !c.is_alphabetic());
    if is_common(&lower) || (base.chars().count() >= 6 && is_common(base)) {
        return refuse("this password is too common; try a few unrelated words".into());
    }
    let distinct: std::collections::HashSet<&char> = chars.iter().collect();
    if distinct.len() < 5 || is_repeated_unit(&chars) || chars.iter().all(char::is_ascii_digit) {
        return refuse("this password is too easy to guess; try a few unrelated words".into());
    }
    if let Some(u) = username.map(str::to_lowercase).filter(|u| u.len() >= 4) {
        if lower.contains(&u) {
            return refuse("the password can't contain your username".into());
        }
    }
    Ok(())
}

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

    #[test]
    fn the_embedded_list_is_sorted_and_searchable() {
        let lines: Vec<&str> = COMMON_PASSWORDS.lines().collect();
        assert!(lines.len() > 80_000);
        assert!(lines.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
        for w in [
            lines[0],
            lines[1],
            lines[lines.len() / 2],
            lines[lines.len() - 1],
        ] {
            assert!(is_common(w), "{w}");
        }
        assert!(is_common("password") && !is_common("not on any list at all"));
        assert!(!is_common("") && !is_common("zzzzzzzzzzzzzzzzzzzzzz"));
    }

    #[test]
    fn policy_refuses_short_common_and_guessable_passwords() {
        for bad in [
            "elevenchars",     // 11
            "PASSWORD1234",    // common word, padded
            "qwerty123456",    // in the list or padded
            "!!sunshine2026!", // padded on both sides
            "aaaaaaaaaaaa",    // one character
            "abcdefabcdef",    // repeated unit
            "839201746512",    // digits only
        ] {
            assert!(check_policy(bad, None).is_err(), "{bad}");
        }
        assert!(
            check_policy(&"long enough ".repeat(30), None).is_err(),
            "over 256 bytes"
        );
        assert!(check_policy("my-marigold-7-lamp", Some("Marigold")).is_err());
    }

    #[test]
    fn policy_accepts_a_passphrase_and_counts_characters_not_bytes() {
        assert!(check_policy("correct horse battery", Some("admin")).is_ok());
        assert!(check_policy("my-marigold-7-lamp", Some("sam")).is_ok());
        assert!(check_policy("ñandú güero ő", None).is_ok(), "13 characters");
        assert!(
            check_policy("ñandú güero", None).is_err(),
            "11 characters, 14 bytes"
        );
    }
}
