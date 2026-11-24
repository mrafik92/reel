-- Second sign-in factor (#146): TOTP, single-use recovery codes and passkeys.
CREATE TABLE user_totp (
  user_id      INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  -- ChaCha20-Poly1305 (nonce + ciphertext), keyed from the signing key and bound to user_id
  secret       BLOB NOT NULL,
  -- NULL until the first valid code proves the authenticator app has the secret
  confirmed_at INTEGER,
  -- newest 30-second step accepted, so a code can't be replayed
  last_step    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE user_recovery_codes (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  -- keyed hash: the codes are shown once and never stored
  code_hash TEXT NOT NULL,
  used_at   INTEGER,
  UNIQUE (user_id, code_hash)
);

CREATE TABLE user_passkeys (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  credential_id TEXT NOT NULL UNIQUE,
  name          TEXT NOT NULL,
  -- webauthn-rs Passkey as JSON: public key and signature counter, nothing secret
  passkey       TEXT NOT NULL,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  last_used_at  INTEGER
);
CREATE INDEX idx_user_passkeys_user ON user_passkeys(user_id);
