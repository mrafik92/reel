-- Reel initial schema (SPEC §3). Timestamps are unix epoch seconds (INTEGER).
-- Booleans are INTEGER 0/1. *_json columns hold JSON text.

CREATE TABLE users (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,
  role          TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin','user')),
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  disabled      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE user_limits (
  user_id               INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  max_streams           INTEGER,
  max_transcodes        INTEGER,
  max_bitrate_kbps      INTEGER,
  request_quota_movies  INTEGER,
  request_quota_tv      INTEGER,
  quota_window_days     INTEGER NOT NULL DEFAULT 7,
  auto_approve          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE sessions_auth (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash  TEXT NOT NULL UNIQUE,
  device_id   TEXT,
  device_name TEXT,
  client      TEXT,
  ip          TEXT,
  last_seen   INTEGER NOT NULL DEFAULT (unixepoch()),
  expires_at  INTEGER
);
CREATE INDEX idx_sessions_auth_user_id ON sessions_auth(user_id);

CREATE TABLE invites (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  code        TEXT NOT NULL UNIQUE,
  created_by  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  library_ids TEXT NOT NULL DEFAULT '[]',
  limits_json TEXT NOT NULL DEFAULT '{}',
  expires_at  INTEGER,
  used_by     INTEGER REFERENCES users(id) ON DELETE SET NULL,
  used_at     INTEGER
);
CREATE INDEX idx_invites_created_by ON invites(created_by);

CREATE TABLE libraries (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  name          TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('movies','shows')),
  paths_json    TEXT NOT NULL DEFAULT '[]',
  scan_interval INTEGER,
  created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE library_access (
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  PRIMARY KEY (user_id, library_id)
);
CREATE INDEX idx_library_access_library_id ON library_access(library_id);

CREATE TABLE media_items (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  library_id         INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  kind               TEXT NOT NULL CHECK (kind IN ('movie','show','season','episode')),
  parent_id          INTEGER REFERENCES media_items(id) ON DELETE CASCADE,
  title              TEXT NOT NULL,
  sort_title         TEXT,
  year               INTEGER,
  tmdb_id            INTEGER,
  tvdb_id            INTEGER,
  imdb_id            TEXT,
  overview           TEXT,
  rating             REAL,
  content_rating     TEXT,
  runtime_s          INTEGER,
  season_no          INTEGER,
  episode_no         INTEGER,
  air_date           TEXT,
  poster             TEXT,
  backdrop           TEXT,
  added_at           INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at         INTEGER NOT NULL DEFAULT (unixepoch()),
  locked_fields_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX idx_media_items_library_id ON media_items(library_id);
CREATE INDEX idx_media_items_parent_id ON media_items(parent_id);
CREATE INDEX idx_media_items_tmdb_id ON media_items(tmdb_id);
CREATE INDEX idx_media_items_kind_sort ON media_items(library_id, kind, sort_title);

CREATE TABLE media_files (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id    INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  path       TEXT NOT NULL UNIQUE,
  size       INTEGER NOT NULL,
  mtime      INTEGER NOT NULL,
  container  TEXT,
  duration_s REAL,
  bitrate    INTEGER,
  probe_json TEXT,
  edition    TEXT,
  part_no    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_media_files_item_id ON media_files(item_id);

CREATE TABLE streams (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  file_id       INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  idx           INTEGER NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('video','audio','subtitle')),
  codec         TEXT,
  profile       TEXT,
  level         INTEGER,
  width         INTEGER,
  height        INTEGER,
  hdr           TEXT NOT NULL DEFAULT 'none' CHECK (hdr IN ('none','hdr10','hlg','dv')),
  bit_depth     INTEGER,
  channels      INTEGER,
  language      TEXT,
  title         TEXT,
  "default"     INTEGER NOT NULL DEFAULT 0,
  forced        INTEGER NOT NULL DEFAULT 0,
  external_path TEXT
);
CREATE INDEX idx_streams_file_id ON streams(file_id);

CREATE TABLE people (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  tmdb_id INTEGER UNIQUE,
  name    TEXT NOT NULL,
  photo   TEXT
);

CREATE TABLE credits (
  item_id   INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  person_id INTEGER NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  role      TEXT NOT NULL,
  character TEXT,
  ord       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (item_id, person_id, role)
);
CREATE INDEX idx_credits_person_id ON credits(person_id);

CREATE TABLE genres (
  id   INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL UNIQUE
);

CREATE TABLE item_genres (
  item_id  INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  genre_id INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
  PRIMARY KEY (item_id, genre_id)
);
CREATE INDEX idx_item_genres_genre_id ON item_genres(genre_id);

CREATE TABLE collections (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  name    TEXT NOT NULL,
  tmdb_id INTEGER UNIQUE
);

CREATE TABLE collection_items (
  collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  item_id       INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  PRIMARY KEY (collection_id, item_id)
);
CREATE INDEX idx_collection_items_item_id ON collection_items(item_id);

CREATE TABLE watch_state (
  user_id        INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  item_id        INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  position_s     REAL NOT NULL DEFAULT 0,
  played         INTEGER NOT NULL DEFAULT 0,
  play_count     INTEGER NOT NULL DEFAULT 0,
  last_played_at INTEGER,
  PRIMARY KEY (user_id, item_id)
);
CREATE INDEX idx_watch_state_item_id ON watch_state(item_id);

CREATE TABLE watchlist (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  item_id  INTEGER REFERENCES media_items(id) ON DELETE CASCADE,
  tmdb_id  INTEGER,
  added_at INTEGER NOT NULL DEFAULT (unixepoch()),
  CHECK (item_id IS NOT NULL OR tmdb_id IS NOT NULL)
);
CREATE INDEX idx_watchlist_user_id ON watchlist(user_id);
CREATE INDEX idx_watchlist_tmdb_id ON watchlist(tmdb_id);

-- IPTV (declared before playback_sessions, which references channels)
CREATE TABLE iptv_sources (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  name             TEXT NOT NULL,
  kind             TEXT NOT NULL CHECK (kind IN ('m3u','xtream')),
  url              TEXT NOT NULL,
  username         TEXT,
  password         TEXT,
  max_connections  INTEGER,
  epg_url          TEXT,
  refresh_interval INTEGER
);

CREATE TABLE channels (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id     INTEGER NOT NULL REFERENCES iptv_sources(id) ON DELETE CASCADE,
  ext_id        TEXT,
  name          TEXT NOT NULL,
  number        INTEGER,
  group_name    TEXT,
  logo          TEXT,
  tvg_id        TEXT,
  stream_url    TEXT NOT NULL,
  catchup_json  TEXT,
  enabled       INTEGER NOT NULL DEFAULT 1,
  favorite_rank INTEGER
);
CREATE INDEX idx_channels_source_id ON channels(source_id);
CREATE INDEX idx_channels_tvg_id ON channels(tvg_id);

CREATE TABLE channel_aliases (
  channel_id            INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  alt_source_channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  priority              INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (channel_id, alt_source_channel_id)
);
CREATE INDEX idx_channel_aliases_alt ON channel_aliases(alt_source_channel_id);

CREATE TABLE epg_programs (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  channel_tvg_id TEXT NOT NULL,
  start          INTEGER NOT NULL,
  stop           INTEGER NOT NULL,
  title          TEXT NOT NULL,
  sub_title      TEXT,
  description    TEXT,
  category       TEXT,
  icon           TEXT,
  episode_num    TEXT
);
CREATE INDEX idx_epg_channel_time ON epg_programs(channel_tvg_id, start);

CREATE TABLE playback_sessions (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id             INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id           TEXT,
  file_id             INTEGER REFERENCES media_files(id) ON DELETE CASCADE,
  channel_id          INTEGER REFERENCES channels(id) ON DELETE CASCADE,
  mode                TEXT NOT NULL CHECK (mode IN ('direct','remux','transcode')),
  video_decision_json TEXT,
  started_at          INTEGER NOT NULL DEFAULT (unixepoch()),
  last_ping           INTEGER NOT NULL DEFAULT (unixepoch()),
  bitrate             INTEGER,
  CHECK (file_id IS NOT NULL OR channel_id IS NOT NULL)
);
CREATE INDEX idx_playback_sessions_user_id ON playback_sessions(user_id);
CREATE INDEX idx_playback_sessions_file_id ON playback_sessions(file_id);

CREATE TABLE arr_servers (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  kind            TEXT NOT NULL CHECK (kind IN ('sonarr','radarr')),
  name            TEXT NOT NULL,
  url             TEXT NOT NULL,
  api_key         TEXT NOT NULL,
  default_profile INTEGER,
  default_root    TEXT,
  is_4k           INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE download_clients (
  id       INTEGER PRIMARY KEY AUTOINCREMENT,
  kind     TEXT NOT NULL CHECK (kind IN ('qbittorrent','transmission')),
  url      TEXT NOT NULL,
  username TEXT,
  password TEXT
);

CREATE TABLE requests (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id            INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  media_type         TEXT NOT NULL CHECK (media_type IN ('movie','tv')),
  tmdb_id            INTEGER NOT NULL,
  seasons_json       TEXT,
  status             TEXT NOT NULL DEFAULT 'requested' CHECK (status IN
    ('requested','approved','searching','not_found','queued','downloading','stalled','importing','available','failed','declined')),
  arr_kind           TEXT CHECK (arr_kind IN ('sonarr','radarr')),
  arr_id             INTEGER REFERENCES arr_servers(id) ON DELETE SET NULL,
  quality_profile_id INTEGER,
  root_folder        TEXT,
  approved_by        INTEGER REFERENCES users(id) ON DELETE SET NULL,
  created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at         INTEGER NOT NULL DEFAULT (unixepoch()),
  note               TEXT
);
CREATE INDEX idx_requests_user_id ON requests(user_id);
CREATE INDEX idx_requests_tmdb_id ON requests(tmdb_id);
CREATE INDEX idx_requests_status ON requests(status);

CREATE TABLE issues (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  item_id    INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL CHECK (kind IN ('video','audio','subtitle','other')),
  message    TEXT,
  status     TEXT NOT NULL DEFAULT 'open',
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_issues_user_id ON issues(user_id);
CREATE INDEX idx_issues_item_id ON issues(item_id);

CREATE TABLE request_downloads (
  request_id    INTEGER NOT NULL REFERENCES requests(id) ON DELETE CASCADE,
  arr_queue_id  INTEGER,
  download_id   TEXT,
  state         TEXT,
  release_title TEXT,
  size          INTEGER,
  last_progress REAL,
  updated_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (request_id, download_id)
);

CREATE TABLE notification_targets (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL CHECK (kind IN ('discord','telegram','webhook','email')),
  config_json TEXT NOT NULL DEFAULT '{}',
  events_json TEXT NOT NULL DEFAULT '[]'
);
