-- Per-channel guide mode + Xtream VOD catalog.
ALTER TABLE iptv_sources ADD COLUMN epg_mode TEXT NOT NULL DEFAULT 'none';
ALTER TABLE iptv_sources ADD COLUMN vod_movie_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE iptv_sources ADD COLUMN vod_series_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE iptv_sources ADD COLUMN last_vod_at INTEGER;
-- Why the XMLTV guide was not usable (the per-channel guide is used instead).
ALTER TABLE iptv_sources ADD COLUMN epg_note TEXT;

-- Lazily fetched per-channel guide (Xtream get_short_epg): when it was last refreshed.
CREATE TABLE epg_fetch_state (
  channel_id INTEGER PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
  fetched_at INTEGER NOT NULL,
  programmes INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE vod_categories (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id INTEGER NOT NULL REFERENCES iptv_sources(id) ON DELETE CASCADE,
  kind      TEXT NOT NULL CHECK (kind IN ('movie','series')),
  ext_id    TEXT NOT NULL,
  name      TEXT NOT NULL,
  UNIQUE (source_id, kind, ext_id)
);

CREATE TABLE vod_movies (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id     INTEGER NOT NULL REFERENCES iptv_sources(id) ON DELETE CASCADE,
  stream_id     INTEGER NOT NULL,
  category_id   INTEGER REFERENCES vod_categories(id) ON DELETE SET NULL,
  title         TEXT NOT NULL,
  title_norm    TEXT NOT NULL,
  year          INTEGER,
  rating        REAL,
  poster        TEXT,
  backdrop      TEXT,
  container     TEXT,
  added_at      INTEGER,
  tmdb_id       INTEGER,
  genres        TEXT,
  overview      TEXT,
  runtime_s     INTEGER,
  director      TEXT,
  cast_json     TEXT,
  trailer       TEXT,
  info_at       INTEGER,
  missing       INTEGER NOT NULL DEFAULT 0,
  UNIQUE (source_id, stream_id)
);
CREATE INDEX idx_vod_movies_cat ON vod_movies(category_id);
CREATE INDEX idx_vod_movies_tmdb ON vod_movies(tmdb_id);
CREATE INDEX idx_vod_movies_norm ON vod_movies(title_norm);

CREATE TABLE vod_series (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id     INTEGER NOT NULL REFERENCES iptv_sources(id) ON DELETE CASCADE,
  series_id     INTEGER NOT NULL,
  category_id   INTEGER REFERENCES vod_categories(id) ON DELETE SET NULL,
  title         TEXT NOT NULL,
  title_norm    TEXT NOT NULL,
  year          INTEGER,
  rating        REAL,
  poster        TEXT,
  backdrop      TEXT,
  added_at      INTEGER,
  tmdb_id       INTEGER,
  genres        TEXT,
  overview      TEXT,
  director      TEXT,
  cast_json     TEXT,
  trailer       TEXT,
  info_at       INTEGER,
  missing       INTEGER NOT NULL DEFAULT 0,
  UNIQUE (source_id, series_id)
);
CREATE INDEX idx_vod_series_cat ON vod_series(category_id);
CREATE INDEX idx_vod_series_tmdb ON vod_series(tmdb_id);
CREATE INDEX idx_vod_series_norm ON vod_series(title_norm);

CREATE TABLE vod_seasons (
  series_pk INTEGER NOT NULL REFERENCES vod_series(id) ON DELETE CASCADE,
  season_no INTEGER NOT NULL,
  name      TEXT,
  poster    TEXT,
  PRIMARY KEY (series_pk, season_no)
);

CREATE TABLE vod_episodes (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  series_pk  INTEGER NOT NULL REFERENCES vod_series(id) ON DELETE CASCADE,
  ext_id     INTEGER NOT NULL,
  season_no  INTEGER NOT NULL,
  episode_no INTEGER NOT NULL,
  title      TEXT NOT NULL,
  overview   TEXT,
  still      TEXT,
  runtime_s  INTEGER,
  container  TEXT,
  UNIQUE (series_pk, ext_id)
);

CREATE TABLE vod_watch (
  user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind           TEXT NOT NULL CHECK (kind IN ('movie','episode')),
  item_id        INTEGER NOT NULL,
  position_s     REAL NOT NULL DEFAULT 0,
  duration_s     REAL,
  played         INTEGER NOT NULL DEFAULT 0,
  last_played_at INTEGER,
  PRIMARY KEY (user_id, kind, item_id)
);

-- ffprobe results of remote items (probing costs a provider connection: do it once).
CREATE TABLE vod_probe (
  kind       TEXT NOT NULL,
  item_id    INTEGER NOT NULL,
  probe_json TEXT NOT NULL,
  probed_at  INTEGER NOT NULL,
  PRIMARY KEY (kind, item_id)
);
