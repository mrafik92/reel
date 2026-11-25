-- Library trailers (#302): the YouTube id of a movie's or show's main trailer, from TMDB's videos.
-- `trailer_checked_at` records when TMDB was last asked, so titles without a trailer are not asked
-- again on every pass of the startup backfill (`reel_metadata::trailers`).
ALTER TABLE media_items ADD COLUMN trailer_youtube_id TEXT;
ALTER TABLE media_items ADD COLUMN trailer_checked_at INTEGER;

CREATE INDEX idx_media_items_trailer_backfill
ON media_items (trailer_checked_at, id)
WHERE kind IN ('movie', 'show') AND tmdb_id IS NOT NULL AND trailer_youtube_id IS NULL;
