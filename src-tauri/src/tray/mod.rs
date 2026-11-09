use std::sync::Mutex;
use tauri::image::Image;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::classifier::LOW_CONFIDENCE_MAX;
use crate::media::{ArtistFlag, NowPlayingChanged};

const TRAY_ID: &str = "main";
const IDLE_TOOLTIP: &str = "Now Playing — AI-Artist Flagger: idle";
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

const ITEM_AUTOSTART: &str = "autostart";
const ITEM_QUIT: &str = "quit";

/// Label for the autostart checkbox, named after what the OS itself calls it. On Linux the
/// plugin writes an XDG `~/.config/autostart/*.desktop` entry, which runs at login.
#[cfg(windows)]
const AUTOSTART_LABEL: &str = "Start with Windows";
#[cfg(not(windows))]
const AUTOSTART_LABEL: &str = "Start on login";

/// The menu is rebuilt from scratch on every update (its per-artist submenus come and go with
/// the track), so the current autostart checkbox is tracked here for `toggle_autostart` to
/// flip once the autostart write has actually succeeded.
struct TrayMenuState {
    autostart: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
}

/// Tray icon + menu + tooltip setup (docs/phase0-plan.md §4.3). The tray is the *only*
/// surface in Phase 1 -- no notifications, no overlay. Left-click-to-open-menu is
/// unsupported on Linux, so all info lives in the standard right-click menu.
pub fn setup(app: &AppHandle) -> anyhow::Result<()> {
    app.manage(TrayMenuState {
        autostart: Mutex::new(None),
    });
    let menu = build_menu(app, None)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(IDLE_ICON)?)
        .menu(&menu)
        .tooltip(IDLE_TOOLTIP)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ITEM_AUTOSTART => toggle_autostart(app),
            ITEM_QUIT => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// Builds the whole menu for the given now-playing state:
///
/// ```text
/// Track — Artist A, Artist B        (label)
/// Flagged: possible AI artist       (label)
/// ─────
/// Artist A: Flagged             ▸   per-signal explanation lines (classifier::explain)
/// Artist B: Not flagged         ▸
/// ─────
/// [x] Start on login / Start with Windows
/// ─────
/// Quit
/// ```
fn build_menu(
    app: &AppHandle,
    state: Option<&NowPlayingChanged>,
) -> tauri::Result<Menu<tauri::Wry>> {
    let (track_text, status_text, _, _) = summarize(state);
    let menu = Menu::new(app)?;

    menu.append(&MenuItem::new(
        app,
        menu_text(&track_text),
        false,
        None::<&str>,
    )?)?;
    if !status_text.is_empty() {
        menu.append(&MenuItem::new(app, status_text, false, None::<&str>)?)?;
    }

    if let Some(state) = state.filter(|s| !s.artists.is_empty()) {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for artist in &state.artists {
            let label = menu_text(&format!("{}: {}", artist.name, artist_status(artist)));
            if artist.reasons.is_empty() {
                menu.append(&MenuItem::new(app, label, false, None::<&str>)?)?;
            } else {
                let submenu = Submenu::new(app, label, true)?;
                for line in &artist.reasons {
                    // Enabled (but inert) so the text renders at full contrast rather than
                    // grayed out -- these lines are the point of the submenu.
                    submenu.append(&MenuItem::new(app, menu_text(line), true, None::<&str>)?)?;
                }
                menu.append(&submenu)?;
            }
        }
    }

    // Reflects the actual autostart state (registry on Windows, XDG autostart entry on Linux)
    // rather than assuming unchecked, so a toggle made outside this menu still shows correctly.
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        ITEM_AUTOSTART,
        AUTOSTART_LABEL,
        true,
        autostart_enabled,
        None::<&str>,
    )?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&autostart)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        ITEM_QUIT,
        "Quit",
        true,
        None::<&str>,
    )?)?;

    if let Some(menu_state) = app.try_state::<TrayMenuState>() {
        *menu_state.autostart.lock().unwrap() = Some(autostart);
    }
    Ok(menu)
}

fn artist_status(artist: &ArtistFlag) -> &'static str {
    match artist.is_flagged {
        None => "Checking…",
        Some(true) => "Flagged",
        Some(false) if artist.confidence.is_some_and(|c| c <= LOW_CONFIDENCE_MAX) => "Uncertain",
        Some(false) => "Not flagged",
    }
}

/// Menu labels treat a single `&` as a keyboard-mnemonic marker on every platform (muda maps
/// it to Windows' native `&` / GTK's `_`) and swallow it, which used to turn "Earth, Wind &
/// Fire" into "Earth, Wind  Fire". `&&` renders as a literal `&`.
fn menu_text(text: &str) -> String {
    text.replace('&', "&&")
}

/// Flips the actual autostart registration and, only on success, the checkbox to match --
/// on failure the checkbox is left as-is (still reflecting reality) rather than showing a
/// state the autostart write didn't actually achieve.
fn toggle_autostart(app: &AppHandle) {
    let currently_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let result = if currently_enabled {
        app.autolaunch().disable()
    } else {
        app.autolaunch().enable()
    };
    if let Err(err) = result {
        log::warn!("failed to toggle autostart: {err:?}");
        return;
    }
    if let Some(menu_state) = app.try_state::<TrayMenuState>()
        && let Some(item) = menu_state.autostart.lock().unwrap().as_ref()
    {
        let _ = item.set_checked(!currently_enabled);
    }
}

/// Called by `media::MediaMonitor` whenever the now-playing state changes. Swaps the tray
/// icon between idle/pending/clear/flagged/unresolved -- that swap *is* the flagged signal
/// (docs/phase0-plan.md §4.3) -- and refreshes the tooltip and the menu.
pub fn update(app: &AppHandle, state: Option<&NowPlayingChanged>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let (_, _, tooltip, tray_state) = summarize(state);

    // On Linux this sets the AppIndicator title instead (see vendor/tray-icon/PATCHED.md),
    // which Cinnamon and similar desktops show on hover.
    let _ = tray.set_tooltip(Some(tooltip.as_str()));
    if let Ok(image) = Image::from_bytes(tray_state.icon_bytes()) {
        let _ = tray.set_icon(Some(image));
    }

    match build_menu(app, state) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => log::warn!("failed to rebuild tray menu: {err:?}"),
    }
}

fn summarize(state: Option<&NowPlayingChanged>) -> (String, String, String, TrayState) {
    let Some(state) = state else {
        return (
            "Nothing playing".to_string(),
            String::new(),
            IDLE_TOOLTIP.to_string(),
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

/// `NOTIFYICONDATA::szTip` is a 128-WCHAR buffer, but the `tray-icon` crate never opts in to
/// `NOTIFYICON_VERSION_4` / `NIF_SHOWTIP` (it doesn't set `uVersion` at all), so Windows falls
/// back to the pre-Vista tooltip behavior: the shell displays only the first 64 characters of
/// szTip and silently drops the rest, regardless of how much the buffer actually holds. A long
/// track/artist line would otherwise truncate the tail of the string, which is exactly the
/// "Flagged" status line since it's written last. Reserve room for `status_text` in full
/// (within that real 64-char budget) and truncate `track_text` instead -- the status is the
/// one thing that must never be cut off.
fn build_tooltip(track_text: &str, status_text: &str) -> String {
    const MAX_UTF16_LEN: usize = 63; // 64-char legacy szTip display limit, minus the NUL terminator.

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

    fn artist(is_flagged: Option<bool>, confidence: Option<f64>) -> ArtistFlag {
        ArtistFlag {
            artist_id: "name:test".to_string(),
            name: "Test".to_string(),
            is_flagged,
            confidence,
            reasons: Vec::new(),
        }
    }

    fn state(artists: Vec<ArtistFlag>) -> NowPlayingChanged {
        NowPlayingChanged {
            track_title: "Song".to_string(),
            spotify_track_id: None,
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
    fn menu_text_keeps_ampersands_literal() {
        assert_eq!(menu_text("Earth, Wind & Fire"), "Earth, Wind && Fire");
    }

    #[test]
    fn artist_status_labels() {
        assert_eq!(artist_status(&artist(None, None)), "Checking…");
        assert_eq!(artist_status(&artist(Some(true), Some(1.0))), "Flagged");
        assert_eq!(artist_status(&artist(Some(false), Some(0.2))), "Uncertain");
        assert_eq!(
            artist_status(&artist(Some(false), Some(1.0))),
            "Not flagged"
        );
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
        assert!(tooltip.encode_utf16().count() <= 63);
    }
}
