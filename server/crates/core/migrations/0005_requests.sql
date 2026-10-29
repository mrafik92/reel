-- Requests crate: remember the Arr-side id (Radarr movieId / Sonarr seriesId) and display metadata.
ALTER TABLE requests ADD COLUMN arr_item_id INTEGER;
ALTER TABLE requests ADD COLUMN title TEXT;
ALTER TABLE requests ADD COLUMN poster TEXT;
CREATE INDEX idx_requests_arr_item ON requests(arr_kind, arr_item_id);
