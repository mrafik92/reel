-- Per guide: how many of the source's channels match a channel of that guide (whether or not an
-- earlier guide ended up supplying them).
ALTER TABLE iptv_epg_sources ADD COLUMN available INTEGER NOT NULL DEFAULT 0;
