-- Several external guide (XMLTV) URLs per source, with per-guide import statistics.
-- `epg_url` stays as a deprecated alias of the first entry.
ALTER TABLE iptv_sources ADD COLUMN epg_urls TEXT;
UPDATE iptv_sources SET epg_urls = json_array(epg_url) WHERE epg_url IS NOT NULL AND epg_url <> '';

CREATE TABLE iptv_epg_sources (
  source_id  INTEGER NOT NULL REFERENCES iptv_sources(id) ON DELETE CASCADE,
  rank       INTEGER NOT NULL,           -- 0 = the provider's own guide, 1.. = epg_urls in order
  url        TEXT NOT NULL,              -- credentials redacted
  provider   INTEGER NOT NULL DEFAULT 0,
  channels   INTEGER NOT NULL DEFAULT 0, -- guide channels this guide delivered
  programmes INTEGER NOT NULL DEFAULT 0, -- programmes stored from it
  matched    INTEGER NOT NULL DEFAULT 0, -- this source's channels linked to one of its channels
  last_error TEXT,
  last_at    INTEGER,
  PRIMARY KEY (source_id, rank)
);

-- which guide a guide channel came from (earlier guides win when several carry a channel)
ALTER TABLE epg_channels ADD COLUMN guide_source INTEGER;
ALTER TABLE epg_channels ADD COLUMN rank INTEGER NOT NULL DEFAULT 0;
