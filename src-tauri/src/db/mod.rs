pub mod artist_classification;
pub mod classification_evidence;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use artist_classification::{ArtistClassification, NewClassification};
use classification_evidence::NewEvidence;

const MIGRATIONS: &[(&str, &str)] =
    &[("0001_init", include_str!("../../migrations/0001_init.sql"))];

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        run_migrations(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_classification(
        &self,
        artist_id: &str,
    ) -> anyhow::Result<Option<ArtistClassification>> {
        let conn = self.conn.lock().unwrap();
        Ok(artist_classification::get(&conn, artist_id)?)
    }

    pub fn recent_classifications(&self, limit: i64) -> anyhow::Result<Vec<ArtistClassification>> {
        let conn = self.conn.lock().unwrap();
        Ok(artist_classification::recent(&conn, limit)?)
    }

    /// Single BEGIN/COMMIT over both tables (docs/phase0-plan.md §3.1 step 6) so a crash
    /// never leaves a classification row written without its evidence row, or vice versa.
    pub fn write_classification_with_evidence(
        &self,
        classification: &NewClassification,
        evidence: &NewEvidence,
    ) -> anyhow::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        artist_classification::insert(&tx, classification)?;
        classification_evidence::insert(&tx, evidence)?;
        tx.commit()?;
        Ok(())
    }
}

fn run_migrations(conn: &mut Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL)",
    )?;
    for (name, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?;
        if already_applied {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO _migrations (name, applied_at) VALUES (?1, ?2)",
            rusqlite::params![name, chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
    }
    Ok(())
}
