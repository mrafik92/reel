-- Sign-in through an OpenID Connect provider (#149).
CREATE TABLE oidc_providers (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  slug          TEXT NOT NULL UNIQUE,
  name          TEXT NOT NULL,
  issuer        TEXT NOT NULL,
  discovery_url TEXT NOT NULL,
  client_id     TEXT NOT NULL,
  -- ChaCha20-Poly1305, keyed from the signing key and bound to the provider id
  client_secret BLOB,
  enabled       INTEGER NOT NULL DEFAULT 1,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

-- An identity is (issuer, subject), never an email address.
CREATE TABLE user_identities (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider_id  INTEGER NOT NULL REFERENCES oidc_providers(id) ON DELETE CASCADE,
  issuer       TEXT NOT NULL,
  subject      TEXT NOT NULL,
  -- shown in Settings so people can tell accounts apart; never used to look anyone up
  label        TEXT,
  created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  last_used_at INTEGER,
  UNIQUE (issuer, subject)
);
CREATE INDEX idx_user_identities_user ON user_identities(user_id);
