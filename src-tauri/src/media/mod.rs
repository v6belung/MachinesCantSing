#[cfg(target_os = "linux")]
mod mpris;
#[cfg(windows)]
mod smtc;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc::UnboundedSender;

use crate::classifier::{self, ClassifyRequest};
use crate::db::Db;
use crate::itunes::ItunesClient;
use crate::text::normalize_artist_name;
use crate::tray;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

/// The normalized event a platform backend emits. `None` means no active track
/// (stopped/paused/no session) -- the tray goes idle (docs/phase0-plan.md §2.2).
#[derive(Debug, Clone)]
pub struct RawNowPlaying {
    pub track_title: String,
    /// The raw, unsplit artist credit exactly as reported by the OS (e.g. "Earth, Wind &
    /// Fire"). Evaluated as its own candidate alongside `artist_names` below, since splitting
    /// is a heuristic that's wrong for any real artist whose name itself contains "," or "&".
    pub artist_credit: String,
    /// Best-effort split into individual artist names (heuristic on Windows/SMTC; native on
    /// Linux/MPRIS, which reports artists as a structured list already).
    pub artist_names: Vec<String>,
    // Part of the normalized event shape per docs/phase0-plan.md §2.1; Stopped is already
    // filtered to `None` by backends before this struct is built, so Playing vs Paused isn't
    // consumed downstream yet -- kept for backends/consumers that need the distinction later.
    #[allow(dead_code)]
    pub playback_status: PlaybackStatus,
}

pub type RawEventSender = UnboundedSender<Option<RawNowPlaying>>;

/// One trait, two backends (SMTC on Windows, MPRIS on Linux) -- the rest of the
/// app consumes one normalized event regardless of platform (docs/phase0-plan.md §2.1).
pub trait MediaBackend: Send {
    /// Start monitoring on a dedicated thread; sends events until the sender is dropped.
    fn spawn(self: Box<Self>, tx: RawEventSender);
}

#[cfg(windows)]
pub fn platform_backend() -> Box<dyn MediaBackend> {
    Box::new(smtc::SmtcBackend::new())
}

#[cfg(target_os = "linux")]
pub fn platform_backend() -> Box<dyn MediaBackend> {
    Box::new(mpris::MprisBackend::new())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn platform_backend() -> Box<dyn MediaBackend> {
    compile_error!("now-playing-flagger only supports Windows (SMTC) and Linux (MPRIS)");
}

/// artist_id = "name:" + normalize(artist_name) (docs/phase0-plan.md §2.3). Computable
/// offline from the media event alone, so the artist_classification PK lookup IS the
/// dedup cache -- no separate "seen" set needed.
pub fn artist_id(artist_name: &str) -> String {
    format!("name:{}", normalize_artist_name(artist_name))
}

/// The names to classify for a track: the raw, unsplit credit line plus each individually
/// split name, deduped by normalized identity. Evaluating both catches two different failure
/// modes of the split heuristic -- a real artist whose name contains "," or "&" is still
/// classified correctly under the whole-line entry even if the split mangles it, while a
/// genuine collab still gets each participant classified individually. A track is flagged if
/// *any* candidate resolves flagged (docs/phase0-plan.md's OR-across-candidates refinement).
fn candidate_names(artist_credit: &str, artist_names: &[String]) -> Vec<String> {
    let mut candidates = Vec::with_capacity(artist_names.len() + 1);
    let mut seen = HashSet::new();

    for name in std::iter::once(artist_credit).chain(artist_names.iter().map(String::as_str)) {
        if seen.insert(normalize_artist_name(name)) {
            candidates.push(name.to_string());
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_line_and_splits_are_both_candidates() {
        let candidates = candidate_names(
            "Earth, Wind & Fire",
            &["Earth".to_string(), "Wind".to_string(), "Fire".to_string()],
        );
        assert_eq!(
            candidates,
            vec!["Earth, Wind & Fire", "Earth", "Wind", "Fire"]
        );
    }

    #[test]
    fn single_artist_track_has_no_duplicate_candidate() {
        let candidates = candidate_names("Some Artist", &["Some Artist".to_string()]);
        assert_eq!(candidates, vec!["Some Artist"]);
    }

    #[test]
    fn single_artist_dedup_is_normalization_insensitive() {
        let candidates = candidate_names("Beyoncé", &["beyonce".to_string()]);
        assert_eq!(candidates, vec!["Beyoncé"]);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtistFlag {
    pub artist_id: String,
    pub name: String,
    /// null/"pending" while a brand-new artist is still being classified.
    pub is_flagged: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NowPlayingChanged {
    pub track_title: String,
    pub artists: Vec<ArtistFlag>,
}

/// Owns the dedup PK check and the in-flight guard, drives the classifier pipeline for
/// never-before-seen artists, and emits `now-playing-changed` (docs/phase0-plan.md §2.3, §4.1).
pub struct MediaMonitor {
    app_handle: AppHandle,
    db: Arc<Db>,
    itunes: Arc<ItunesClient>,
    last_state: Mutex<Option<NowPlayingChanged>>,
    in_flight: Mutex<HashSet<String>>,
}

impl MediaMonitor {
    pub fn new(app_handle: AppHandle, db: Arc<Db>, itunes: Arc<ItunesClient>) -> Self {
        Self {
            app_handle,
            db,
            itunes,
            last_state: Mutex::new(None),
            in_flight: Mutex::new(HashSet::new()),
        }
    }

    pub fn current_state(&self) -> Option<NowPlayingChanged> {
        self.last_state.lock().unwrap().clone()
    }

    /// Starts the platform backend thread plus the async event-consumer task.
    pub fn start(self: Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        platform_backend().spawn(tx);

        tauri::async_runtime::spawn(async move {
            while let Some(raw) = rx.recv().await {
                self.clone().handle_raw_event(raw).await;
            }
        });
    }

    async fn handle_raw_event(self: Arc<Self>, raw: Option<RawNowPlaying>) {
        let Some(raw) = raw else {
            *self.last_state.lock().unwrap() = None;
            self.emit_and_update_tray(None);
            return;
        };

        let candidates = candidate_names(&raw.artist_credit, &raw.artist_names);

        let mut artists = Vec::with_capacity(candidates.len());
        for name in &candidates {
            let id = artist_id(name);
            let known = self.db.get_classification(&id).ok().flatten();
            let is_flagged = known.map(|row| row.is_flagged);
            artists.push(ArtistFlag {
                artist_id: id.clone(),
                name: name.clone(),
                is_flagged,
            });

            if is_flagged.is_none() && self.mark_in_flight(&id) {
                self.clone().spawn_classification(ClassifyRequest {
                    artist_id: id,
                    artist_name: name.clone(),
                    track_title: raw.track_title.clone(),
                });
            }
        }

        let state = NowPlayingChanged {
            track_title: raw.track_title,
            artists,
        };
        *self.last_state.lock().unwrap() = Some(state.clone());
        self.emit_and_update_tray(Some(state));
    }

    fn mark_in_flight(&self, id: &str) -> bool {
        self.in_flight.lock().unwrap().insert(id.to_string())
    }

    fn clear_in_flight(&self, id: &str) {
        self.in_flight.lock().unwrap().remove(id);
    }

    fn spawn_classification(self: Arc<Self>, req: ClassifyRequest) {
        tauri::async_runtime::spawn(async move {
            let artist_id = req.artist_id.clone();
            let artist_name = req.artist_name.clone();
            if let Err(err) = classifier::classify(&self.db, &self.itunes, req).await {
                log::error!("classification failed for '{artist_name}': {err:?}");
            }
            self.clear_in_flight(&artist_id);
            self.apply_classification_result(&artist_id);
        });
    }

    /// Re-reads the just-classified artist's verdict from the DB and, if it's still part
    /// of the currently-displayed track, patches it in and re-emits -- without clobbering
    /// a track change that may have happened while classification was in flight.
    fn apply_classification_result(&self, artist_id: &str) {
        let mut guard = self.last_state.lock().unwrap();
        let Some(state) = guard.as_mut() else {
            return;
        };
        let Some(artist) = state.artists.iter_mut().find(|a| a.artist_id == artist_id) else {
            return;
        };
        if let Ok(Some(row)) = self.db.get_classification(artist_id) {
            artist.is_flagged = Some(row.is_flagged);
        }
        let snapshot = Some(state.clone());
        drop(guard);
        self.emit_and_update_tray(snapshot);
    }

    fn emit_and_update_tray(&self, state: Option<NowPlayingChanged>) {
        tray::update(&self.app_handle, state.as_ref());
        let _ = self.app_handle.emit("now-playing-changed", state);
    }
}
