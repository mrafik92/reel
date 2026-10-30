-- Per-user playback preferences (audio/subtitle languages …), stored as JSON.
CREATE TABLE user_preferences (
  user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  json    TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
