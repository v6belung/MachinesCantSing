use std::sync::{Arc, Mutex as StdMutex};
use std::thread;
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as WinPlaybackStatus,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::RuntimeType;
use windows_future::IAsyncOperation;

/// Blocks the calling (non-async) thread until a WinRT `IAsyncOperation<T>` completes.
/// `windows-future` 0.3 dropped the old synchronous `.get()` in favor of `IntoFuture`/`.when()`;
/// this rebuilds a blocking wait on top of `.when()` for use from the dedicated OS thread the
/// SMTC backend runs on (no tokio/async context there).
fn wait_for<T: RuntimeType + Send + 'static>(op: IAsyncOperation<T>) -> anyhow::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    op.when(move |result| {
        let _ = tx.send(result);
    })?;
    let result = rx
        .recv()
        .map_err(|_| anyhow::anyhow!("WinRT async operation channel closed before completion"))?;
    Ok(result?)
}

use super::{MediaBackend, PlaybackStatus, RawEventSender, RawNowPlaying};

pub struct SmtcBackend;

impl SmtcBackend {
    pub fn new() -> Self {
        Self
    }
}

impl MediaBackend for SmtcBackend {
    fn spawn(self: Box<Self>, tx: RawEventSender) {
        thread::spawn(move || {
            if let Err(err) = run(tx) {
                log::error!("SMTC media session monitor stopped: {err:?}");
            }
        });
    }
}

struct CurrentSession {
    session: Session,
    props_token: i64,
    playback_token: i64,
}

struct SmtcState {
    tx: RawEventSender,
    manager: SessionManager,
    current: StdMutex<Option<CurrentSession>>,
}

fn run(tx: RawEventSender) -> anyhow::Result<()> {
    // MTA: WinRT event callbacks land on threadpool threads without needing a Win32
    // message loop, unlike the STA the webview's main thread already runs.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }

    let manager = wait_for(SessionManager::RequestAsync()?)?;
    let state = Arc::new(SmtcState {
        tx,
        manager: manager.clone(),
        current: StdMutex::new(None),
    });

    state.refresh();

    let sessions_changed_state = state.clone();
    let _sessions_token = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        sessions_changed_state.refresh();
        Ok(())
    }))?;

    // Event delivery happens on WinRT threadpool threads; just keep this thread (and its
    // MTA + the `state`/`manager` it owns) alive for the life of the process.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

impl SmtcState {
    /// Re-picks the Spotify session (sessions may have been added/removed) and, if it
    /// changed, tears down the old per-session subscriptions and wires up the new one.
    /// Always followed by emitting the current snapshot.
    fn refresh(self: &Arc<Self>) {
        let spotify = find_spotify_session(&self.manager);

        {
            let mut current = self.current.lock().unwrap();
            let is_same = match (&*current, &spotify) {
                (Some(c), Some(s)) => aumid(&c.session) == aumid(s),
                (None, None) => true,
                _ => false,
            };

            if !is_same {
                if let Some(old) = current.take() {
                    let _ = old.session.RemoveMediaPropertiesChanged(old.props_token);
                    let _ = old.session.RemovePlaybackInfoChanged(old.playback_token);
                }
                if let Some(session) = spotify {
                    let props_state = self.clone();
                    let props_token =
                        session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                            props_state.emit_current();
                            Ok(())
                        }));
                    let playback_state = self.clone();
                    let playback_token =
                        session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                            playback_state.emit_current();
                            Ok(())
                        }));
                    if let (Ok(props_token), Ok(playback_token)) = (props_token, playback_token) {
                        *current = Some(CurrentSession {
                            session,
                            props_token,
                            playback_token,
                        });
                    }
                }
            }
        }

        self.emit_current();
    }

    fn emit_current(self: &Arc<Self>) {
        let current = self.current.lock().unwrap();
        let raw = match current.as_ref() {
            Some(c) => read_now_playing(&c.session).unwrap_or(None),
            None => None,
        };
        let _ = self.tx.send(raw);
    }
}

fn aumid(session: &Session) -> String {
    session
        .SourceAppUserModelId()
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn find_spotify_session(manager: &SessionManager) -> Option<Session> {
    let sessions = manager.GetSessions().ok()?;
    let count = sessions.Size().ok()?;
    for i in 0..count {
        if let Ok(session) = sessions.GetAt(i)
            && aumid(&session).to_lowercase().contains("spotify")
        {
            return Some(session);
        }
    }
    None
}

fn read_now_playing(session: &Session) -> anyhow::Result<Option<RawNowPlaying>> {
    let playback_status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    let status = match playback_status {
        WinPlaybackStatus::Playing => PlaybackStatus::Playing,
        WinPlaybackStatus::Paused => PlaybackStatus::Paused,
        _ => PlaybackStatus::Stopped,
    };

    if status == PlaybackStatus::Stopped {
        return Ok(None);
    }

    let props = wait_for(session.TryGetMediaPropertiesAsync()?)?;
    let title = props.Title()?.to_string();
    let artist = props.Artist()?.to_string();

    if title.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(RawNowPlaying {
        track_title: title,
        artist_names: split_artists(&artist),
        playback_status: status,
    }))
}

/// Spotify's own SMTC formatting convention for multi-artist credits is "A, B & C" --
/// split on that rather than a generic delimiter set, since a blind comma-split would
/// mangle single-artist names that legitimately contain a comma.
fn split_artists(artist: &str) -> Vec<String> {
    artist
        .split(", ")
        .flat_map(|part| part.split(" & "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
