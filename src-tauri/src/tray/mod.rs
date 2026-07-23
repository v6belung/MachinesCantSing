use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::classifier::LOW_CONFIDENCE_MAX;
use crate::media::NowPlayingChanged;

const TRAY_ID: &str = "main";
/// Nothing playing.
const IDLE_ICON: &[u8] = include_bytes!("../../icons/tray-neutral.png");
/// At least one candidate artist still classifying.
const PENDING_ICON: &[u8] = include_bytes!("../../icons/tray-pending.png");
/// All candidates resolved, none flagged, none unresolved.
const CLEAR_ICON: &[u8] = include_bytes!("../../icons/tray-clear.png");
/// At least one candidate resolved flagged.
const FLAGGED_ICON: &[u8] = include_bytes!("../../icons/tray-flagged.png");
/// No candidate flagged, but at least one couldn't be confidently resolved on iTunes.
const UNRESOLVED_ICON: &[u8] = include_bytes!("../../icons/tray-unresolved.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayState {
    Idle,
    Pending,
    Clear,
    Flagged,
    Unresolved,
}

impl TrayState {
    fn icon_bytes(self) -> &'static [u8] {
        match self {
            TrayState::Idle => IDLE_ICON,
            TrayState::Pending => PENDING_ICON,
            TrayState::Clear => CLEAR_ICON,
            TrayState::Flagged => FLAGGED_ICON,
            TrayState::Unresolved => UNRESOLVED_ICON,
        }
    }
}

const ITEM_CURRENT_TRACK: &str = "current-track";
const ITEM_FLAGGED_STATUS: &str = "flagged-status";
const ITEM_OPEN_WINDOW: &str = "open-window";
const ITEM_QUIT: &str = "quit";

/// The two label-only menu items get their text updated in place by `update()`. Kept in
/// managed state rather than round-tripped through the tray's menu, since that's simpler
/// and avoids re-deriving menu item identity every update.
struct TrayMenuItems {
    current_track: MenuItem<tauri::Wry>,
    flagged_status: MenuItem<tauri::Wry>,
}

/// Tray icon + menu + tooltip setup (docs/phase0-plan.md §4.3). The tray is the *only*
/// surface in Phase 1 -- no notifications, no overlay. Left-click-to-open-menu is
/// unsupported on Linux, so all info lives in the standard right-click menu.
pub fn setup(app: &AppHandle) -> anyhow::Result<()> {
    let current_track = MenuItem::with_id(
        app,
        ITEM_CURRENT_TRACK,
        "Nothing playing",
        false,
        None::<&str>,
    )?;
    let flagged_status = MenuItem::with_id(app, ITEM_FLAGGED_STATUS, "", false, None::<&str>)?;
    let open_window = MenuItem::with_id(
        app,
        ITEM_OPEN_WINDOW,
        "Open status window",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, ITEM_QUIT, "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &current_track,
            &flagged_status,
            &PredefinedMenuItem::separator(app)?,
            &open_window,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(IDLE_ICON)?)
        .menu(&menu)
        .tooltip("Now Playing — AI-Artist Flagger: idle")
        .on_menu_event(|app, event| match event.id().as_ref() {
            ITEM_OPEN_WINDOW => {
                if let Some(window) = app.get_webview_window("status") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            ITEM_QUIT => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayMenuItems {
        current_track,
        flagged_status,
    });

    Ok(())
}

/// Called by `media::MediaMonitor` whenever the now-playing state changes. Swaps the tray
/// icon between idle/pending/clear/flagged/unresolved -- that swap *is* the flagged signal
/// (docs/phase0-plan.md §4.3) -- and refreshes the tooltip and the two label-only menu items.
pub fn update(app: &AppHandle, state: Option<&NowPlayingChanged>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let (track_text, status_text, tooltip, tray_state) = summarize(state);

    let _ = tray.set_tooltip(Some(tooltip.as_str()));
    if let Ok(image) = Image::from_bytes(tray_state.icon_bytes()) {
        let _ = tray.set_icon(Some(image));
    }

    if let Some(items) = app.try_state::<TrayMenuItems>() {
        let _ = items.current_track.set_text(track_text);
        let _ = items.flagged_status.set_text(status_text);
    }
}

fn summarize(state: Option<&NowPlayingChanged>) -> (String, String, String, TrayState) {
    let Some(state) = state else {
        return (
            "Nothing playing".to_string(),
            String::new(),
            "Now Playing — AI-Artist Flagger: idle".to_string(),
            TrayState::Idle,
        );
    };

    let names = state
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    // Multiple candidates per track (whole credit line + each split artist, see
    // media::candidate_names): flagged if *any* candidate is flagged. Priority when
    // candidates disagree: still-classifying beats everything (transient), a confirmed
    // flag beats an unresolved lookup (don't let "couldn't check artist B" hide "artist A
    // is flagged"), and unresolved only shows once nothing is pending or flagged.
    let pending = state.artists.iter().any(|a| a.is_flagged.is_none());
    let flagged = state.artists.iter().any(|a| a.is_flagged == Some(true));
    let unresolved = state.artists.iter().any(|a| {
        a.is_flagged == Some(false) && a.confidence.is_some_and(|c| c <= LOW_CONFIDENCE_MAX)
    });
    let (status_text, tray_state) = if pending {
        ("Classifying…".to_string(), TrayState::Pending)
    } else if flagged {
        (
            "Flagged: possible AI artist".to_string(),
            TrayState::Flagged,
        )
    } else if unresolved {
        (
            "Not flagged (unresolved lookup)".to_string(),
            TrayState::Unresolved,
        )
    } else {
        ("Not flagged".to_string(), TrayState::Clear)
    };
    let track_text = format!("{} — {}", state.track_title, names);
    let tooltip = build_tooltip(&track_text, &status_text);

    (track_text, status_text, tooltip, tray_state)
}

/// Windows tray tooltips are capped at 128 UTF-16 code units (`NOTIFYICONDATA::szTip`); a
/// long track/artist line would otherwise silently truncate the tail of the string, which is
/// exactly the "Flagged" status line since it's written last. Reserve room for `status_text`
/// in full and truncate `track_text` instead -- the status is the one thing that must never
/// be cut off.
fn build_tooltip(track_text: &str, status_text: &str) -> String {
    const MAX_UTF16_LEN: usize = 127; // 128-length buffer, minus the NUL terminator.

    let suffix = format!("\n{status_text}");
    let suffix_len = suffix.encode_utf16().count();
    let budget = MAX_UTF16_LEN.saturating_sub(suffix_len);

    if track_text.encode_utf16().count() <= budget {
        return format!("{track_text}{suffix}");
    }

    let ellipsis_budget = budget.saturating_sub(1); // room for the trailing "…"
    let mut truncated = String::new();
    let mut len = 0usize;
    for c in track_text.chars() {
        let w = c.len_utf16();
        if len + w > ellipsis_budget {
            break;
        }
        len += w;
        truncated.push(c);
    }
    truncated.push('…');

    format!("{truncated}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ArtistFlag;

    fn artist(is_flagged: Option<bool>, confidence: Option<f64>) -> ArtistFlag {
        ArtistFlag {
            artist_id: "name:test".to_string(),
            name: "Test".to_string(),
            is_flagged,
            confidence,
        }
    }

    fn state(artists: Vec<ArtistFlag>) -> NowPlayingChanged {
        NowPlayingChanged {
            track_title: "Song".to_string(),
            artists,
        }
    }

    #[test]
    fn unresolved_shown_when_no_flags_and_low_confidence() {
        let s = state(vec![artist(Some(false), Some(0.2))]);
        let (_, status, _, tray_state) = summarize(Some(&s));
        assert_eq!(tray_state, TrayState::Unresolved);
        assert_eq!(status, "Not flagged (unresolved lookup)");
    }

    #[test]
    fn confidently_clear_is_not_unresolved() {
        let s = state(vec![artist(Some(false), Some(1.0))]);
        let (_, _, _, tray_state) = summarize(Some(&s));
        assert_eq!(tray_state, TrayState::Clear);
    }

    #[test]
    fn flagged_takes_priority_over_unresolved() {
        let s = state(vec![
            artist(Some(true), Some(1.0)),
            artist(Some(false), Some(0.2)),
        ]);
        let (_, _, _, tray_state) = summarize(Some(&s));
        assert_eq!(tray_state, TrayState::Flagged);
    }

    #[test]
    fn pending_takes_priority_over_flagged_and_unresolved() {
        let s = state(vec![
            artist(None, None),
            artist(Some(true), Some(1.0)),
            artist(Some(false), Some(0.2)),
        ]);
        let (_, _, _, tray_state) = summarize(Some(&s));
        assert_eq!(tray_state, TrayState::Pending);
    }

    #[test]
    fn short_tooltip_is_unchanged() {
        assert_eq!(
            build_tooltip("Song — Artist", "Not flagged"),
            "Song — Artist\nNot flagged"
        );
    }

    #[test]
    fn long_track_text_is_truncated_but_status_survives_in_full() {
        let long_track = "A ".repeat(80) + "— Artist With A Very Long Name Indeed";
        let tooltip = build_tooltip(&long_track, "Flagged: possible AI artist");
        assert!(tooltip.ends_with("\nFlagged: possible AI artist"));
        assert!(tooltip.encode_utf16().count() <= 127);
    }
}
