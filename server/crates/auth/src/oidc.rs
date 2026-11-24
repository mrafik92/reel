//! OpenID Connect ID token verification (#149). Only RS256 and ES256 are accepted, and the key type
//! has to match the algorithm, so `none` and HMAC-with-the-public-key tokens can't get through.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature;
use serde::Deserialize;
use serde_json::Value;

/// Clock difference tolerated between the provider and this server.
pub const CLOCK_SKEW_S: i64 = 60;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwks {
    #[serde(default)]
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwk {
    #[serde(default)]
    pub kty: String,
    pub kid: Option<String>,
    pub alg: Option<String>,
    #[serde(rename = "use")]
    pub usage: Option<String>,
    pub n: Option<String>,
    pub e: Option<String>,
    pub crv: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
}

/// What a token must match.
#[derive(Debug, Clone, Copy)]
pub struct IdTokenCheck<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdClaims {
    pub subject: String,
    /// For display only. Never used to find or link an account.
    pub email: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("malformed token")]
    Malformed,
    #[error("algorithm not allowed")]
    Algorithm,
    /// The provider may have rotated its keys: refresh the key set once and retry.
    #[error("unknown signing key")]
    UnknownKey,
    #[error("bad signature")]
    Signature,
    #[error("wrong issuer")]
    Issuer,
    #[error("wrong audience")]
    Audience,
    #[error("expired or not yet valid")]
    Time,
    #[error("wrong nonce")]
    Nonce,
}

fn b64(s: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| TokenError::Malformed)
}

fn find_key<'a>(jwks: &'a Jwks, kid: Option<&str>, alg: &str) -> Result<&'a Jwk, TokenError> {
    let kty = if alg == "RS256" { "RSA" } else { "EC" };
    let usable = |k: &&Jwk| {
        k.kty == kty
            && k.usage.as_deref().is_none_or(|u| u == "sig")
            && k.alg.as_deref().is_none_or(|a| a == alg)
    };
    match kid {
        Some(id) => jwks
            .keys
            .iter()
            .filter(usable)
            .find(|k| k.kid.as_deref() == Some(id))
            .ok_or(TokenError::UnknownKey),
        // without a `kid` the choice must be unambiguous
        None => {
            let mut it = jwks.keys.iter().filter(usable);
            match (it.next(), it.next()) {
                (Some(k), None) => Ok(k),
                _ => Err(TokenError::UnknownKey),
            }
        }
    }
}

fn verify_signature(key: &Jwk, alg: &str, msg: &[u8], sig: &[u8]) -> Result<(), TokenError> {
    let bad = |_| TokenError::Signature;
    if alg == "RS256" {
        let (n, e) = key
            .n
            .as_deref()
            .zip(key.e.as_deref())
            .ok_or(TokenError::UnknownKey)?;
        let (n, e) = (b64(n)?, b64(e)?);
        // this ring parameter set refuses RSA keys under 2048 bits
        return signature::RsaPublicKeyComponents { n: &n, e: &e }
            .verify(&signature::RSA_PKCS1_2048_8192_SHA256, msg, sig)
            .map_err(bad);
    }
    if key.crv.as_deref() != Some("P-256") {
        return Err(TokenError::UnknownKey);
    }
    let (x, y) = key
        .x
        .as_deref()
        .zip(key.y.as_deref())
        .ok_or(TokenError::UnknownKey)?;
    let (x, y) = (b64(x)?, b64(y)?);
    if x.len() != 32 || y.len() != 32 {
        return Err(TokenError::UnknownKey);
    }
    let mut point = Vec::with_capacity(65);
    point.push(4);
    point.extend_from_slice(&x);
    point.extend_from_slice(&y);
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point)
        .verify(msg, sig)
        .map_err(bad)
}

fn check_audience(claims: &Value, client_id: &str) -> Result<(), TokenError> {
    let multiple = match &claims["aud"] {
        Value::String(a) if a == client_id => false,
        Value::Array(list) if list.iter().any(|a| a == client_id) => list.len() > 1,
        _ => return Err(TokenError::Audience),
    };
    match claims.get("azp").and_then(Value::as_str) {
        Some(azp) if azp != client_id => Err(TokenError::Audience),
        None if multiple => Err(TokenError::Audience),
        _ => Ok(()),
    }
}

fn check_time(claims: &Value, now: i64) -> Result<(), TokenError> {
    let exp = claims["exp"].as_i64().ok_or(TokenError::Time)?;
    if exp + CLOCK_SKEW_S <= now {
        return Err(TokenError::Time);
    }
    for field in ["iat", "nbf"] {
        if claims[field]
            .as_i64()
            .is_some_and(|t| t > now + CLOCK_SKEW_S)
        {
            return Err(TokenError::Time);
        }
    }
    Ok(())
}

/// Verify an ID token's signature and claims.
pub fn verify_id_token(
    token: &str,
    jwks: &Jwks,
    check: &IdTokenCheck<'_>,
) -> Result<IdClaims, TokenError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(TokenError::Malformed);
    }
    let parts: Vec<&str> = token.split('.').collect();
    let [header, payload, sig] = parts[..] else {
        return Err(TokenError::Malformed);
    };
    let head: Value = serde_json::from_slice(&b64(header)?).map_err(|_| TokenError::Malformed)?;
    let alg = head["alg"].as_str().ok_or(TokenError::Malformed)?;
    if !matches!(alg, "RS256" | "ES256") {
        return Err(TokenError::Algorithm);
    }
    let key = find_key(jwks, head["kid"].as_str(), alg)?;
    let signed_len = header.len() + 1 + payload.len();
    verify_signature(key, alg, &token.as_bytes()[..signed_len], &b64(sig)?)?;

    let claims: Value =
        serde_json::from_slice(&b64(payload)?).map_err(|_| TokenError::Malformed)?;
    if claims["iss"].as_str() != Some(check.issuer) {
        return Err(TokenError::Issuer);
    }
    check_audience(&claims, check.client_id)?;
    check_time(&claims, check.now)?;
    if claims["nonce"].as_str() != Some(check.nonce) {
        return Err(TokenError::Nonce);
    }
    let subject = claims["sub"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 255)
        .ok_or(TokenError::Malformed)?
        .to_string();
    Ok(IdClaims {
        subject,
        email: claims["email"]
            .as_str()
            .map(|e| e.chars().filter(|c| !c.is_control()).take(254).collect()),
    })
}

#[cfg(test)]
mod tests {
    use ring::{
        rand::SystemRandom,
        signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
    };
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000;
    const CHECK: IdTokenCheck<'static> = IdTokenCheck {
        issuer: "https://issuer.example",
        client_id: "reel-client",
        nonce: "nonce-1",
        now: NOW,
    };

    struct Signer {
        pair: EcdsaKeyPair,
        rng: SystemRandom,
    }

    impl Signer {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            Self { pair, rng }
        }

        fn jwks(&self, kid: &str) -> Jwks {
            let p = self.pair.public_key().as_ref();
            Jwks {
                keys: vec![Jwk {
                    kty: "EC".into(),
                    kid: Some(kid.into()),
                    crv: Some("P-256".into()),
                    x: Some(URL_SAFE_NO_PAD.encode(&p[1..33])),
                    y: Some(URL_SAFE_NO_PAD.encode(&p[33..65])),
                    ..Default::default()
                }],
            }
        }

        fn token_with(&self, head: Value, claims: Value) -> String {
            let msg = format!(
                "{}.{}",
                URL_SAFE_NO_PAD.encode(head.to_string()),
                URL_SAFE_NO_PAD.encode(claims.to_string())
            );
            let sig = self.pair.sign(&self.rng, msg.as_bytes()).unwrap();
            format!("{msg}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }

        fn token(&self, claims: Value) -> String {
            self.token_with(json!({"alg": "ES256", "kid": "k1"}), claims)
        }
    }

    fn good() -> Value {
        json!({
            "iss": CHECK.issuer, "aud": CHECK.client_id, "sub": "subject-1", "nonce": CHECK.nonce,
            "exp": NOW + 300, "iat": NOW, "email": "person@example.com",
        })
    }

    fn with(field: &str, v: Value) -> Value {
        let mut c = good();
        c[field] = v;
        c
    }

    #[test]
    fn a_good_token_is_accepted() {
        let s = Signer::new();
        let c = verify_id_token(&s.token(good()), &s.jwks("k1"), &CHECK).unwrap();
        assert_eq!(c.subject, "subject-1");
        assert_eq!(c.email.as_deref(), Some("person@example.com"));
    }

    #[test]
    fn claims_are_enforced() {
        let s = Signer::new();
        let j = s.jwks("k1");
        for (claims, want) in [
            (
                with("iss", json!("https://evil.example")),
                TokenError::Issuer,
            ),
            (with("aud", json!("someone-else")), TokenError::Audience),
            (
                with("aud", json!(["reel-client", "other"])),
                TokenError::Audience,
            ),
            (with("azp", json!("other")), TokenError::Audience),
            (with("exp", json!(NOW - CLOCK_SKEW_S)), TokenError::Time),
            (with("iat", json!(NOW + CLOCK_SKEW_S + 1)), TokenError::Time),
            (with("nbf", json!(NOW + CLOCK_SKEW_S + 1)), TokenError::Time),
            (with("nonce", json!("replayed")), TokenError::Nonce),
            (with("sub", json!("")), TokenError::Malformed),
        ] {
            assert_eq!(verify_id_token(&s.token(claims), &j, &CHECK), Err(want));
        }
        let mut no_exp = good();
        no_exp.as_object_mut().unwrap().remove("exp");
        assert_eq!(
            verify_id_token(&s.token(no_exp), &j, &CHECK),
            Err(TokenError::Time)
        );
    }

    #[test]
    fn small_clock_skew_is_tolerated_and_multi_audience_needs_azp() {
        let s = Signer::new();
        let j = s.jwks("k1");
        assert!(verify_id_token(&s.token(with("exp", json!(NOW - 30))), &j, &CHECK).is_ok());
        let mut c = with("aud", json!(["reel-client", "other"]));
        c["azp"] = json!("reel-client");
        assert!(verify_id_token(&s.token(c), &j, &CHECK).is_ok());
    }

    #[test]
    fn signatures_and_algorithms_are_enforced() {
        let s = Signer::new();
        let other = Signer::new();
        let j = s.jwks("k1");
        assert_eq!(
            verify_id_token(&other.token(good()), &j, &CHECK),
            Err(TokenError::Signature)
        );
        // payload swapped under a valid signature
        let t = s.token(good());
        let forged = URL_SAFE_NO_PAD.encode(with("sub", json!("admin")).to_string());
        let p: Vec<&str> = t.split('.').collect();
        assert_eq!(
            verify_id_token(&format!("{}.{forged}.{}", p[0], p[2]), &j, &CHECK),
            Err(TokenError::Signature)
        );
        for alg in ["none", "HS256", "RS512"] {
            let t = s.token_with(json!({"alg": alg, "kid": "k1"}), good());
            assert_eq!(
                verify_id_token(&t, &j, &CHECK),
                Err(TokenError::Algorithm),
                "{alg}"
            );
        }
        // an EC key can't be used for an RS256 header
        let t = s.token_with(json!({"alg": "RS256", "kid": "k1"}), good());
        assert_eq!(verify_id_token(&t, &j, &CHECK), Err(TokenError::UnknownKey));
        let t = s.token_with(json!({"alg": "ES256", "kid": "rotated"}), good());
        assert_eq!(verify_id_token(&t, &j, &CHECK), Err(TokenError::UnknownKey));
        for junk in ["", "a.b", "a.b.c.d", "!!.??.##"] {
            assert_eq!(
                verify_id_token(junk, &j, &CHECK),
                Err(TokenError::Malformed)
            );
        }
        assert_eq!(
            verify_id_token(&"a".repeat(MAX_TOKEN_BYTES + 1), &j, &CHECK),
            Err(TokenError::Malformed)
        );
    }
}
