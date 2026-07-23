use tauri::State;

use crate::AppState;
use crate::db::artist_classification::ArtistClassification;
use crate::media::{MediaMonitor, NowPlayingChanged};

/// Last-known now-playing snapshot + flagged status of its artists, so the status
/// window isn't blank between events (docs/phase0-plan.md §4.1).
#[tauri::command]
pub fn get_current_state(state: State<AppState>) -> Option<NowPlayingChanged> {
    let monitor: &MediaMonitor = &state.monitor;
    monitor.current_state()
}

#[tauri::command]
pub fn get_recent_classifications(
    state: State<AppState>,
    limit: i64,
) -> Result<Vec<ArtistClassification>, String> {
    state
        .db
        .recent_classifications(limit)
        .map_err(|e| e.to_string())
}
