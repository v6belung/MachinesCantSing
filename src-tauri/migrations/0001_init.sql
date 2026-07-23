CREATE TABLE artist_classification (
    artist_id             TEXT PRIMARY KEY,        -- "name:" + normalized name
    artist_name           TEXT NOT NULL,           -- original display name
    is_flagged            INTEGER NOT NULL,        -- 0/1
    classified_at         TEXT NOT NULL,           -- ISO8601
    method                TEXT NOT NULL,           -- 'itunes_search_api'
    confidence            REAL,
    earliest_release_date TEXT,                    -- ISO8601 date, nullable
    locked                INTEGER NOT NULL DEFAULT 0  -- unused in Phase 1
);

CREATE TABLE classification_evidence (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    artist_id     TEXT NOT NULL,
    source        TEXT NOT NULL,       -- 'itunes_search_api' in Phase 1
    result        TEXT NOT NULL,       -- evidence JSON (see docs/phase0-plan.md §3.3)
    supports_ai   INTEGER NOT NULL,    -- 0/1, mirrors is_flagged for this check
    recorded_at   TEXT NOT NULL,       -- ISO8601
    FOREIGN KEY (artist_id) REFERENCES artist_classification(artist_id)
);
