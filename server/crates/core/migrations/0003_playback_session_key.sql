-- Link live playback sessions (UUID from the playback engine) to their DB row.
ALTER TABLE playback_sessions ADD COLUMN session_key TEXT;
CREATE UNIQUE INDEX idx_playback_sessions_key ON playback_sessions(session_key);
