-- OMDb responses (IMDb / Rotten Tomatoes / Metacritic scores), cached 24 h by the server.
CREATE TABLE omdb_cache (
  imdb_id    TEXT PRIMARY KEY,
  json       TEXT NOT NULL,
  fetched_at INTEGER NOT NULL
);
