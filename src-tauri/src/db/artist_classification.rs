use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ArtistClassification {
    pub artist_id: String,
    pub artist_name: String,
    pub is_flagged: bool,
    pub classified_at: String,
    pub method: String,
    pub confidence: Option<f64>,
    pub earliest_release_date: Option<String>,
    pub locked: bool,
}

pub struct NewClassification<'a> {
    pub artist_id: &'a str,
    pub artist_name: &'a str,
    pub is_flagged: bool,
    pub classified_at: &'a str,
    pub method: &'a str,
    pub confidence: Option<f64>,
    pub earliest_release_date: Option<&'a str>,
}

const COLUMNS: &str = "artist_id, artist_name, is_flagged, classified_at, method, confidence, earliest_release_date, locked";

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<ArtistClassification> {
    Ok(ArtistClassification {
        artist_id: row.get(0)?,
        artist_name: row.get(1)?,
        is_flagged: row.get::<_, i64>(2)? != 0,
        classified_at: row.get(3)?,
        method: row.get(4)?,
        confidence: row.get(5)?,
        earliest_release_date: row.get(6)?,
        locked: row.get::<_, i64>(7)? != 0,
    })
}

pub fn get(conn: &Connection, artist_id: &str) -> rusqlite::Result<Option<ArtistClassification>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM artist_classification WHERE artist_id = ?1"),
        [artist_id],
        from_row,
    )
    .optional()
}

/// Never overwrites: an artist_id that already has a row is permanent (see docs/phase0-plan.md §2.3).
/// A duplicate INSERT fails loudly rather than silently overwriting a prior verdict.
pub fn insert(conn: &Connection, row: &NewClassification) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO artist_classification
         (artist_id, artist_name, is_flagged, classified_at, method, confidence, earliest_release_date, locked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)",
        params![
            row.artist_id,
            row.artist_name,
            row.is_flagged as i64,
            row.classified_at,
            row.method,
            row.confidence,
            row.earliest_release_date,
        ],
    )?;
    Ok(())
}

pub fn recent(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<ArtistClassification>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM artist_classification ORDER BY classified_at DESC LIMIT ?1"
    ))?;
    let rows = stmt.query_map([limit], from_row)?;
    rows.collect()
}
