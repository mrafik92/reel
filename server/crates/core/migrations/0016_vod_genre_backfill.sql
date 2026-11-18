ALTER TABLE vod_movies ADD COLUMN genre_checked_at INTEGER;

CREATE INDEX idx_vod_movies_genre_backfill
ON vod_movies (genre_checked_at, id)
WHERE missing = 0 AND COALESCE(genres, '') = '';
