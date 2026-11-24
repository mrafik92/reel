-- Several logins on one Xtream provider (#297): the same channel or title imported by a second
-- login is flagged as a duplicate of the first login's row and hidden from listings. Playback of
-- the visible row can still use the other login's connection. Recomputed by
-- `reel_iptv::dedup::refresh` after every catalog refresh, source change and server start.
ALTER TABLE channels ADD COLUMN duplicate INTEGER NOT NULL DEFAULT 0;
ALTER TABLE vod_movies ADD COLUMN duplicate INTEGER NOT NULL DEFAULT 0;
ALTER TABLE vod_series ADD COLUMN duplicate INTEGER NOT NULL DEFAULT 0;
