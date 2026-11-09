//! Linux MPRIS backend: reads Spotify's now-playing state from the session D-Bus
//! (`org.mpris.MediaPlayer2.spotify`, interface `org.mpris.MediaPlayer2.Player`) via zbus.
//!
//! Design: every interesting bus event is treated only as a *trigger* -- the backend then
//! re-reads `PlaybackStatus` + `Metadata` from Spotify directly and emits the snapshot if it
//! differs from the last one sent. Spotify's Linux client is known to send partial/duplicate
//! `PropertiesChanged` payloads around track changes, so the payload itself is never trusted.
//! Triggers are:
//! - `PropertiesChanged` on `/org/mpris/MediaPlayer2` (track/status changes),
//! - `NameOwnerChanged` for any MPRIS name (Spotify starting or quitting -- it routinely starts
//!   *after* this app, e.g. when both autostart at login),
//! - a slow safety-net poll, in case Spotify drops a change notification entirely.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use super::{MediaBackend, PlaybackStatus, RawEventSender, RawNowPlaying};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const SPOTIFY_BUS_NAME: &str = "org.mpris.MediaPlayer2.spotify";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
/// Spotify sends bursts of notifications around a track change, sometimes with the new title
/// but not yet the new artist. Waiting briefly before re-reading lets the burst settle.
const SETTLE_DELAY: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait Player {
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

pub struct MprisBackend;

impl MprisBackend {
    pub fn new() -> Self {
        Self
    }
}

impl MediaBackend for MprisBackend {
    fn spawn(self: Box<Self>, tx: RawEventSender) {
        tauri::async_runtime::spawn(async move {
            // Survives across reconnects so a reconnect doesn't re-emit an unchanged state.
            let mut last_sent: Option<Option<RawNowPlaying>> = None;
            while !tx.is_closed() {
                if let Err(err) = run(&tx, &mut last_sent).await {
                    log::error!("MPRIS monitor error, reconnecting: {err:?}");
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        });
    }
}

async fn run(
    tx: &RawEventSender,
    last_sent: &mut Option<Option<RawNowPlaying>>,
) -> anyhow::Result<()> {
    let conn = Connection::session().await?;
    let dbus = DBusProxy::new(&conn).await?;

    let mut owner_changes = dbus.receive_name_owner_changed().await?;
    let props_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path(MPRIS_PATH)?
        .build();
    let mut props_changes = MessageStream::for_match_rule(props_rule, &conn, Some(64)).await?;
    let mut poll = tokio::time::interval(POLL_INTERVAL);

    log::info!("MPRIS monitor connected to session bus");

    loop {
        tokio::select! {
            // First tick fires immediately, which doubles as the initial snapshot.
            _ = poll.tick() => {}
            signal = owner_changes.next() => {
                let Some(signal) = signal else {
                    anyhow::bail!("NameOwnerChanged stream ended");
                };
                let is_mpris = signal
                    .args()
                    .map(|a| a.name().as_str().starts_with(MPRIS_PREFIX))
                    .unwrap_or(false);
                if !is_mpris {
                    continue;
                }
                tokio::time::sleep(SETTLE_DELAY).await;
            }
            msg = props_changes.next() => {
                match msg {
                    Some(Ok(_)) => tokio::time::sleep(SETTLE_DELAY).await,
                    Some(Err(err)) => {
                        log::debug!("bad PropertiesChanged message: {err:?}");
                        continue;
                    }
                    None => anyhow::bail!("PropertiesChanged stream ended"),
                }
            }
        }

        let snapshot = match read_spotify(&conn, &dbus).await {
            Snapshot::Ready(raw) => raw,
            // Mid-transition read: keep showing whatever was last sent; the next trigger or
            // poll tick will pick up the settled state.
            Snapshot::Incomplete => continue,
        };

        if last_sent.as_ref() != Some(&snapshot) {
            *last_sent = Some(snapshot.clone());
            if tx.send(snapshot).is_err() {
                return Ok(()); // consumer gone; the outer loop sees the closed channel and exits
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Snapshot {
    /// A settled state to emit (`None` = nothing to classify: no Spotify, stopped, ad, podcast).
    Ready(Option<RawNowPlaying>),
    /// Spotify is mid-transition or briefly unreadable; don't emit anything.
    Incomplete,
}

async fn read_spotify(conn: &Connection, dbus: &DBusProxy<'_>) -> Snapshot {
    let Some(bus_name) = find_spotify_name(dbus).await else {
        return Snapshot::Ready(None);
    };

    // Uncached: every read must hit Spotify, since its change signals aren't trusted.
    let player = match PlayerProxy::builder(conn).destination(bus_name.as_str()) {
        Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
        Err(err) => Err(err),
    };
    let player = match player {
        Ok(p) => p,
        Err(err) => {
            log::debug!("couldn't build MPRIS proxy for {bus_name}: {err:?}");
            return Snapshot::Incomplete;
        }
    };

    let (status, metadata) = match (player.playback_status().await, player.metadata().await) {
        (Ok(s), Ok(m)) => (s, m),
        (Err(err), _) | (_, Err(err)) => {
            // Usually Spotify having just quit or still starting up.
            log::debug!("couldn't read MPRIS properties from {bus_name}: {err:?}");
            return Snapshot::Incomplete;
        }
    };

    to_snapshot(&status, &SpotifyMetadata::from_mpris(&metadata))
}

/// Spotify registers as `org.mpris.MediaPlayer2.spotify` (deb, snap and flatpak alike); the
/// `.instanceNNN` suffix form is accepted too, as the MPRIS spec allows it for multi-instance
/// players.
async fn find_spotify_name(dbus: &DBusProxy<'_>) -> Option<String> {
    let names = dbus.list_names().await.ok()?;
    names
        .iter()
        .map(|n| n.as_str())
        .find(|n| is_spotify_name(n))
        .map(str::to_string)
}

fn is_spotify_name(name: &str) -> bool {
    name == SPOTIFY_BUS_NAME
        || name
            .strip_prefix(SPOTIFY_BUS_NAME)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The handful of MPRIS metadata fields this app uses, pulled out of the raw `a{sv}` map.
#[derive(Debug, Default, PartialEq)]
struct SpotifyMetadata {
    title: Option<String>,
    artists: Vec<String>,
    track_id: Option<String>,
}

impl SpotifyMetadata {
    fn from_mpris(map: &HashMap<String, OwnedValue>) -> Self {
        Self {
            title: map.get("xesam:title").and_then(|v| value_as_string(v)),
            artists: map
                .get("xesam:artist")
                .map(|v| value_as_string_list(v))
                .unwrap_or_default(),
            track_id: map.get("mpris:trackid").and_then(|v| value_as_string(v)),
        }
    }
}

/// Accepts plain strings and object paths (Spotify has used both for `mpris:trackid` across
/// versions), unwrapping any extra variant layer.
fn value_as_string(value: &Value<'_>) -> Option<String> {
    match value {
        Value::Str(s) => Some(s.as_str().to_string()),
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        Value::Value(inner) => value_as_string(inner),
        _ => None,
    }
}

/// `xesam:artist` is spec'd as `as`, but tolerate a bare string from non-conforming versions.
fn value_as_string_list(value: &Value<'_>) -> Vec<String> {
    match value {
        Value::Array(items) => items.inner().iter().filter_map(value_as_string).collect(),
        Value::Value(inner) => value_as_string_list(inner),
        other => value_as_string(other).into_iter().collect(),
    }
}

/// What kind of item a Spotify track ID refers to: `track`, `ad`, `episode`, ... Spotify has
/// used both `spotify:<kind>:<id>` strings and `/com/spotify/<kind>/<id>` object paths.
fn track_kind(track_id: &str) -> Option<&str> {
    if let Some(rest) = track_id.strip_prefix("spotify:") {
        return rest.split(':').next();
    }
    if let Some(rest) = track_id.strip_prefix("/com/spotify/") {
        return rest.split('/').next();
    }
    None
}

/// The base62 ID of a regular track (not an ad/episode), from either track-ID format.
fn spotify_track_id(track_id: &str) -> Option<String> {
    if track_kind(track_id) != Some("track") {
        return None;
    }
    let id = track_id.rsplit([':', '/']).next()?;
    crate::spotify::is_valid_track_id(id).then(|| id.to_string())
}

fn to_snapshot(status: &str, meta: &SpotifyMetadata) -> Snapshot {
    let status = match status {
        "Playing" => PlaybackStatus::Playing,
        "Paused" => PlaybackStatus::Paused,
        _ => PlaybackStatus::Stopped,
    };
    if status == PlaybackStatus::Stopped {
        return Snapshot::Ready(None);
    }

    // Ads and podcast episodes aren't artists; never classify them.
    if let Some(kind) = meta.track_id.as_deref().and_then(track_kind)
        && matches!(kind, "ad" | "episode")
    {
        return Snapshot::Ready(None);
    }

    let title = meta.title.as_deref().map(str::trim).unwrap_or_default();
    if title.is_empty() {
        return Snapshot::Ready(None);
    }

    let artists: Vec<String> = meta
        .artists
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect();
    // A title with no artist yet is Spotify mid-update, not a real artist-less track.
    let Some(first) = artists.first().cloned() else {
        return Snapshot::Incomplete;
    };

    // The credit line is the *first* artist, not a synthetic "A, B" join: `candidate_names`
    // classifies the credit line as its own identity, and a joined string would be looked up
    // as a single (nonexistent) artist and likely flagged as invisible to iTunes/MusicBrainz.
    // Every listed artist is still classified individually via `artist_names`.
    Snapshot::Ready(Some(RawNowPlaying {
        track_title: title.to_string(),
        artist_credit: first,
        artist_names: artists,
        playback_status: status,
        spotify_track_id: meta.track_id.as_deref().and_then(spotify_track_id),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::ObjectPath;

    fn owned(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
    }

    fn meta(title: &str, artists: &[&str], track_id: Option<&str>) -> SpotifyMetadata {
        SpotifyMetadata {
            title: Some(title.to_string()),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            track_id: track_id.map(str::to_string),
        }
    }

    fn ready(s: Snapshot) -> Option<RawNowPlaying> {
        match s {
            Snapshot::Ready(r) => r,
            Snapshot::Incomplete => panic!("expected Ready, got Incomplete"),
        }
    }

    #[test]
    fn parses_spotify_metadata_map() {
        let mut map = HashMap::new();
        map.insert("xesam:title".to_string(), owned(Value::from("Song")));
        map.insert(
            "xesam:artist".to_string(),
            owned(Value::from(vec!["A", "B"])),
        );
        map.insert(
            "mpris:trackid".to_string(),
            owned(Value::from(
                ObjectPath::try_from("/com/spotify/track/abc123").unwrap(),
            )),
        );
        assert_eq!(
            SpotifyMetadata::from_mpris(&map),
            meta("Song", &["A", "B"], Some("/com/spotify/track/abc123"))
        );
    }

    #[test]
    fn tolerates_bare_string_artist() {
        let mut map = HashMap::new();
        map.insert("xesam:artist".to_string(), owned(Value::from("Solo")));
        assert_eq!(SpotifyMetadata::from_mpris(&map).artists, vec!["Solo"]);
    }

    #[test]
    fn multi_artist_credit_is_first_artist_and_all_are_names() {
        let raw = ready(to_snapshot("Playing", &meta("Song", &["A", "B"], None))).unwrap();
        assert_eq!(raw.artist_credit, "A");
        assert_eq!(raw.artist_names, vec!["A", "B"]);
        assert_eq!(raw.playback_status, PlaybackStatus::Playing);
    }

    #[test]
    fn track_id_extracted_from_both_formats() {
        for id in [
            "/com/spotify/track/4Vw6c5OhtMZY5roKr7EQ0B",
            "spotify:track:4Vw6c5OhtMZY5roKr7EQ0B",
        ] {
            let raw = ready(to_snapshot("Playing", &meta("Song", &["A"], Some(id)))).unwrap();
            assert_eq!(
                raw.spotify_track_id.as_deref(),
                Some("4Vw6c5OhtMZY5roKr7EQ0B")
            );
        }
        assert_eq!(spotify_track_id("/com/spotify/track/short"), None);
    }

    #[test]
    fn paused_keeps_track() {
        let raw = ready(to_snapshot("Paused", &meta("Song", &["A"], None))).unwrap();
        assert_eq!(raw.playback_status, PlaybackStatus::Paused);
    }

    #[test]
    fn stopped_is_nothing_playing() {
        assert_eq!(
            ready(to_snapshot("Stopped", &meta("Song", &["A"], None))),
            None
        );
    }

    #[test]
    fn ads_and_episodes_are_skipped_in_both_id_formats() {
        for id in [
            "spotify:ad:123",
            "/com/spotify/ad/123",
            "spotify:episode:123",
            "/com/spotify/episode/123",
        ] {
            assert_eq!(
                ready(to_snapshot("Playing", &meta("X", &["Spotify"], Some(id)))),
                None,
                "{id}"
            );
        }
        assert!(
            ready(to_snapshot(
                "Playing",
                &meta("X", &["A"], Some("spotify:track:123"))
            ))
            .is_some()
        );
    }

    #[test]
    fn empty_title_is_nothing_playing() {
        assert_eq!(
            ready(to_snapshot("Playing", &meta("  ", &["A"], None))),
            None
        );
    }

    #[test]
    fn missing_artist_is_incomplete_not_a_blank_artist() {
        assert_eq!(
            to_snapshot("Playing", &meta("Song", &[" "], None)),
            Snapshot::Incomplete
        );
    }

    #[test]
    fn spotify_name_matching() {
        assert!(is_spotify_name("org.mpris.MediaPlayer2.spotify"));
        assert!(is_spotify_name("org.mpris.MediaPlayer2.spotify.instance42"));
        assert!(!is_spotify_name("org.mpris.MediaPlayer2.spotifyd"));
        assert!(!is_spotify_name(
            "org.mpris.MediaPlayer2.firefox.instance_1_113"
        ));
    }
}
