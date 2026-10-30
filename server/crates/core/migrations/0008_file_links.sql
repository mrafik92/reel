-- Multi-episode files (S01E02E03): the file belongs to its first episode via media_files.item_id and
-- is linked to every further episode here, so each episode exists as an item and plays the same file.
CREATE TABLE media_file_links (
  file_id INTEGER NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
  item_id INTEGER NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
  PRIMARY KEY (file_id, item_id)
);
CREATE INDEX idx_media_file_links_item ON media_file_links(item_id);

-- The item a playback session was started for (differs from the file's primary item for episode 2+
-- of a multi-episode file), so progress is credited to the right episode.
ALTER TABLE playback_sessions ADD COLUMN item_id INTEGER;
