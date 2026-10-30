-- Guide import statistics shown in the admin UI (see iptv::ingest::refresh_epg).
ALTER TABLE iptv_sources ADD COLUMN epg_channel_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE iptv_sources ADD COLUMN epg_programme_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE iptv_sources ADD COLUMN epg_matched_channels INTEGER NOT NULL DEFAULT 0;
