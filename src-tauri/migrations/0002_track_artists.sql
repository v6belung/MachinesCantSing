-- Full artist credits per Spotify track, from its public page (src/spotify). Cached so each
-- track's page is fetched at most once; only successful, trustworthy parses are stored.
CREATE TABLE track_artists (
    track_id      TEXT PRIMARY KEY,   -- Spotify base62 track id
    artist_names  TEXT NOT NULL,      -- JSON array of names, in credit order
    fetched_at    TEXT NOT NULL       -- ISO8601
);
