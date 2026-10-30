-- Phase 6: IPTV ingest bookkeeping, EPG channel index, per-user favorites.
ALTER TABLE iptv_sources ADD COLUMN user_agent TEXT;
ALTER TABLE iptv_sources ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE iptv_sources ADD COLUMN detected_epg_url TEXT;
ALTER TABLE iptv_sources ADD COLUMN last_refresh_at INTEGER;
ALTER TABLE iptv_sources ADD COLUMN last_epg_at INTEGER;
ALTER TABLE iptv_sources ADD COLUMN last_error TEXT;
ALTER TABLE iptv_sources ADD COLUMN channel_count INTEGER NOT NULL DEFAULT 0;

-- Resolved XMLTV channel id (tvg-id match, else fuzzy name match, else manual override).
ALTER TABLE channels ADD COLUMN epg_channel_id TEXT;
ALTER TABLE channels ADD COLUMN epg_manual INTEGER NOT NULL DEFAULT 0;
-- Admin edits survive refreshes.
ALTER TABLE channels ADD COLUMN name_custom INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN number_custom INTEGER NOT NULL DEFAULT 0;
-- Channels that vanished from the source stay (stable ids / favorites) but are hidden.
ALTER TABLE channels ADD COLUMN missing INTEGER NOT NULL DEFAULT 0;
ALTER TABLE channels ADD COLUMN missing_since INTEGER;
ALTER TABLE channels ADD COLUMN user_agent TEXT;
ALTER TABLE channels ADD COLUMN referrer TEXT;
ALTER TABLE channels ADD COLUMN tvg_name TEXT;
CREATE UNIQUE INDEX idx_channels_source_ext ON channels(source_id, ext_id);
CREATE INDEX idx_channels_epg ON channels(epg_channel_id);

CREATE TABLE epg_channels (
  id        TEXT PRIMARY KEY,
  name      TEXT NOT NULL,
  norm_name TEXT NOT NULL,
  icon      TEXT
);
CREATE INDEX idx_epg_channels_norm ON epg_channels(norm_name);

CREATE TABLE epg_channel_names (
  epg_id    TEXT NOT NULL REFERENCES epg_channels(id) ON DELETE CASCADE,
  norm_name TEXT NOT NULL,
  PRIMARY KEY (epg_id, norm_name)
);
CREATE INDEX idx_epg_channel_names_norm ON epg_channel_names(norm_name);

CREATE INDEX idx_epg_programs_stop ON epg_programs(stop);

CREATE TABLE user_channel_favorites (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  rank       INTEGER NOT NULL DEFAULT 0,
  added_at   INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (user_id, channel_id)
);
CREATE INDEX idx_user_channel_favorites_channel ON user_channel_favorites(channel_id);
