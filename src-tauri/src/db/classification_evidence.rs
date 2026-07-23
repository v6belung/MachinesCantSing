use rusqlite::{Connection, params};

pub struct NewEvidence<'a> {
    pub artist_id: &'a str,
    pub source: &'a str,
    pub result: &'a str, // evidence JSON, see docs/phase0-plan.md §3.3
    pub supports_ai: bool,
    pub recorded_at: &'a str,
}

pub fn insert(conn: &Connection, row: &NewEvidence) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO classification_evidence (artist_id, source, result, supports_ai, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            row.artist_id,
            row.source,
            row.result,
            row.supports_ai as i64,
            row.recorded_at,
        ],
    )?;
    Ok(())
}
