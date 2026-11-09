#[cfg(target_os = "linux")]
mod mpris;
#[cfg(windows)]
mod smtc;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::AppHandle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::classifier::{self, ClassifyRequest};
use crate::db::Db;
use crate::itunes::ItunesClient;
use crate::musicbrainz::MusicBrainzClient;
use crate::spotify::SpotifyPageClient;
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
#[derive(Debug, Clone, PartialEq)]
pub struct RawNowPlaying {
    pub track_title: String,
    /// The artist credit exactly as reported by the OS. On Windows/SMTC this is a single
    /// string that isn't reliably split into per-artist fragments (see `smtc::read_now_playing`
    /// for why guessing at a split was dropped); on Linux/MPRIS it's the first listed artist,
    /// never a synthetic join (see `mpris::to_snapshot` for why).
    pub artist_credit: String,
    /// Individual artist names, when the backend can actually provide them as a structured
    /// list rather than a guess (native on Linux/MPRIS). On Windows/SMTC this is always just
    /// `[artist_credit]` -- there's no separate splitting to do.
    pub artist_names: Vec<String>,
    // Part of the normalized event shape per docs/phase0-plan.md §2.1; Stopped is already
    // filtered to `None` by backends before this struct is built, so Playing vs Paused isn't
    // consumed downstream yet -- kept for backends/consumers that need the distinction later.
    #[allow(dead_code)]
    pub playback_status: PlaybackStatus,
    /// Spotify's base62 track ID, when the OS exposes it (Linux/MPRIS; never on Windows/SMTC).
    /// Lets `MediaMonitor` look up the track's full artist credits (see `crate::spotify`).
    pub spotify_track_id: Option<String>,
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

/// The names to classify for a track: the credit line plus any additional structured names a
/// backend actually provided, deduped by normalized identity. On Windows/SMTC `artist_names` is
/// always just `[artist_credit]`, so this collapses to one candidate; on a backend that can
/// supply genuinely separate per-artist names (native MPRIS), each still gets classified
/// individually and a track is flagged if *any* candidate resolves flagged
/// (docs/phase0-plan.md's OR-across-candidates refinement).
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

#[derive(Debug, Clone)]
pub struct ArtistFlag {
    pub artist_id: String,
    pub name: String,
    /// null/"pending" while a brand-new artist is still being classified.
    pub is_flagged: Option<bool>,
    /// null while pending; otherwise the stored confidence (docs/phase0-plan.md §3.2).
    /// `is_flagged == Some(false)` with a low confidence means "unresolved on iTunes," not
    /// "confidently checked and clear" -- see `classifier::LOW_CONFIDENCE_MAX`.
    pub confidence: Option<f64>,
    /// Plain-language lines explaining the verdict (see `classifier::explain`); empty while
    /// pending or if the evidence couldn't be read.
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NowPlayingChanged {
    pub track_title: String,
    pub spotify_track_id: Option<String>,
    pub artists: Vec<ArtistFlag>,
}

/// Owns the dedup PK check and the in-flight guard, drives the classifier pipeline for
/// never-before-seen artists, and keeps the tray icon updated (docs/phase0-plan.md §2.3, §4.1).
pub struct MediaMonitor {
    app_handle: AppHandle,
    db: Arc<Db>,
    itunes: Arc<ItunesClient>,
    musicbrainz: Arc<MusicBrainzClient>,
    spotify: Arc<SpotifyPageClient>,
    last_state: Mutex<Option<NowPlayingChanged>>,
    in_flight: Mutex<HashSet<String>>,
    /// Track IDs whose Spotify page lookup failed or was untrustworthy this session -- not
    /// retried until restart, so pause/resume (which re-emits the track) can't hammer the page.
    track_lookup_failed: Mutex<HashSet<String>>,
    /// Re-delivers a track once its full credits are cached; see `spawn_track_lookup`.
    enriched_tx: UnboundedSender<RawNowPlaying>,
    enriched_rx: Mutex<Option<UnboundedReceiver<RawNowPlaying>>>,
}

impl MediaMonitor {
    pub fn new(
        app_handle: AppHandle,
        db: Arc<Db>,
        itunes: Arc<ItunesClient>,
        musicbrainz: Arc<MusicBrainzClient>,
        spotify: Arc<SpotifyPageClient>,
    ) -> Self {
        let (enriched_tx, enriched_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            app_handle,
            db,
            itunes,
            musicbrainz,
            spotify,
            last_state: Mutex::new(None),
            in_flight: Mutex::new(HashSet::new()),
            track_lookup_failed: Mutex::new(HashSet::new()),
            enriched_tx,
            enriched_rx: Mutex::new(Some(enriched_rx)),
        }
    }

    /// Starts the platform backend thread plus the async event-consumer task.
    pub fn start(self: Arc<Self>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        platform_backend().spawn(tx);
        let mut enriched_rx = self
            .enriched_rx
            .lock()
            .unwrap()
            .take()
            .expect("MediaMonitor::start called twice");

        // One consumer for both sources, so events are handled strictly in order: an enriched
        // re-delivery can never race a newer track change and overwrite it.
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    raw = rx.recv() => match raw {
                        Some(raw) => self.clone().handle_raw_event(raw).await,
                        None => break,
                    },
                    Some(raw) = enriched_rx.recv() => {
                        if self.is_current_track(&raw) {
                            self.clone().handle_raw_event(Some(raw)).await;
                        }
                    }
                }
            }
        });
    }

    fn is_current_track(&self, raw: &RawNowPlaying) -> bool {
        self.last_state.lock().unwrap().as_ref().is_some_and(|s| {
            s.spotify_track_id.is_some() && s.spotify_track_id == raw.spotify_track_id
        })
    }

    async fn handle_raw_event(self: Arc<Self>, raw: Option<RawNowPlaying>) {
        let Some(mut raw) = raw else {
            *self.last_state.lock().unwrap() = None;
            self.emit_and_update_tray(None);
            return;
        };

        if let Some(track_id) = raw.spotify_track_id.clone() {
            match self.db.get_track_artists(&track_id) {
                Ok(Some(names)) => raw.artist_names = names,
                Ok(None) => {
                    let failed = self.track_lookup_failed.lock().unwrap().contains(&track_id);
                    if !failed && self.mark_in_flight(&format!("track:{track_id}")) {
                        self.clone().spawn_track_lookup(raw.clone(), track_id);
                    }
                }
                Err(err) => log::warn!("couldn't read cached credits for {track_id}: {err:?}"),
            }
        }

        let candidates = candidate_names(&raw.artist_credit, &raw.artist_names);

        let mut artists = Vec::with_capacity(candidates.len());
        for name in &candidates {
            let id = artist_id(name);
            let known = self.db.get_classification(&id).ok().flatten();
            let (is_flagged, confidence, reasons) = match &known {
                Some(row) => (
                    Some(row.is_flagged),
                    row.confidence,
                    self.reasons(&id, row.is_flagged, row.confidence),
                ),
                None => (None, None, Vec::new()),
            };
            artists.push(ArtistFlag {
                artist_id: id.clone(),
                name: name.clone(),
                is_flagged,
                confidence,
                reasons,
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
            spotify_track_id: raw.spotify_track_id,
            artists,
        };
        *self.last_state.lock().unwrap() = Some(state.clone());
        self.emit_and_update_tray(Some(state));
    }

    fn reasons(&self, artist_id: &str, is_flagged: bool, confidence: Option<f64>) -> Vec<String> {
        match self.db.get_latest_evidence(artist_id) {
            Ok(Some((source, json))) => match serde_json::from_str(&json) {
                Ok(evidence) => {
                    classifier::explain::explain(&source, &evidence, is_flagged, confidence)
                }
                Err(err) => {
                    log::warn!("unreadable evidence for {artist_id}: {err:?}");
                    Vec::new()
                }
            },
            Ok(None) => Vec::new(),
            Err(err) => {
                log::warn!("couldn't load evidence for {artist_id}: {err:?}");
                Vec::new()
            }
        }
    }

    /// Fetches the track's full artist credits from its public Spotify page, caches them, and
    /// re-delivers the track so the extra artists get classified and shown. The track is shown
    /// (with the primary artist) immediately in the meantime; any failure just leaves it that way.
    fn spawn_track_lookup(self: Arc<Self>, raw: RawNowPlaying, track_id: String) {
        tauri::async_runtime::spawn(async move {
            let primary = normalize_artist_name(&raw.artist_credit);
            let names = match self.spotify.track_artists(&track_id).await {
                // Sanity check that the page is about the track we think it is.
                Ok(Some(names)) if names.iter().any(|n| normalize_artist_name(n) == primary) => {
                    Some(names)
                }
                Ok(Some(names)) => {
                    log::warn!(
                        "Spotify page for {track_id} credits {names:?}, not '{}'; ignoring",
                        raw.artist_credit
                    );
                    None
                }
                Ok(None) => {
                    log::warn!("Spotify page for {track_id} had no trustworthy artist list");
                    None
                }
                Err(err) => {
                    log::warn!("Spotify page lookup failed for {track_id}: {err:?}");
                    None
                }
            };

            match names {
                Some(names) => {
                    if let Err(err) = self.db.put_track_artists(&track_id, &names) {
                        log::warn!("couldn't cache credits for {track_id}: {err:?}");
                        self.track_lookup_failed
                            .lock()
                            .unwrap()
                            .insert(track_id.clone());
                    } else if names.len() > 1 {
                        log::info!("full credits for '{}': {names:?}", raw.track_title);
                        let _ = self.enriched_tx.send(raw);
                    }
                }
                None => {
                    self.track_lookup_failed
                        .lock()
                        .unwrap()
                        .insert(track_id.clone());
                }
            }
            self.clear_in_flight(&format!("track:{track_id}"));
        });
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
            if let Err(err) =
                classifier::classify(&self.db, &self.itunes, &self.musicbrainz, req).await
            {
                log::error!("classification failed for '{artist_name}': {err:?}");
            }
            self.clear_in_flight(&artist_id);
            self.apply_classification_result(&artist_id);
        });
    }

    /// Re-reads the just-classified artist's verdict from the DB and, if it's still part
    /// of the currently-displayed track, patches it in and refreshes the tray -- without
    /// clobbering a track change that may have happened while classification was in flight.
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
            artist.confidence = row.confidence;
            artist.reasons = self.reasons(artist_id, row.is_flagged, row.confidence);
        }
        let snapshot = Some(state.clone());
        drop(guard);
        self.emit_and_update_tray(snapshot);
    }

    fn emit_and_update_tray(&self, state: Option<NowPlayingChanged>) {
        // Multi-artist flagging depends entirely on what the OS handed us in `artist_credit`
        // (see `candidate_names`) -- if a collab track's flag looks wrong, the first thing to
        // check is whether every credited artist actually made it into this list at all,
        // before suspecting the OR-across-candidates logic itself.
        if let Some(s) = &state {
            log::info!(
                "now-playing: track=\"{}\" candidates={:?}",
                s.track_title,
                s.artists
                    .iter()
                    .map(|a| (a.name.as_str(), a.is_flagged))
                    .collect::<Vec<_>>()
            );
        }
        tray::update(&self.app_handle, state.as_ref());
    }
}
