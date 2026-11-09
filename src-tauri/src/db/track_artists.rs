use rusqlite::{Connection, OptionalExtension, params};

pub fn get(conn: &Connection, track_id: &str) -> anyhow::Result<Option<Vec<String>>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT artist_names FROM track_artists WHERE track_id = ?1",
            [track_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
}

pub fn insert(
    conn: &Connection,
    track_id: &str,
    artist_names: &[String],
    fetched_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO track_artists (track_id, artist_names, fetched_at) VALUES (?1, ?2, ?3)",
        params![track_id, serde_json::to_string(artist_names)?, fetched_at],
    )?;
    Ok(())
}
