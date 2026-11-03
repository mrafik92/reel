-- Security hardening: signed-URL revocation epoch, per-user feature permissions, absolute session
-- lifetime.
ALTER TABLE users ADD COLUMN sign_epoch INTEGER NOT NULL DEFAULT 0;
-- existing users keep IPTV / VOD / requests; casting to TVs is opt-in (admins always can)
ALTER TABLE users ADD COLUMN iptv_access INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN vod_access INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN can_request INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN can_cast INTEGER NOT NULL DEFAULT 0;
-- invites: permissions of the account they create (NULL = none)
ALTER TABLE invites ADD COLUMN perms_json TEXT;
ALTER TABLE sessions_auth ADD COLUMN abs_expires_at INTEGER;
UPDATE sessions_auth SET abs_expires_at = last_seen + 90 * 86400;
