use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::media::NowPlayingChanged;

const TRAY_ID: &str = "main";
const NEUTRAL_ICON: &[u8] = include_bytes!("../../icons/tray-neutral.png");
const FLAGGED_ICON: &[u8] = include_bytes!("../../icons/tray-flagged.png");

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
        .icon(Image::from_bytes(NEUTRAL_ICON)?)
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
/// icon between neutral/flagged -- that swap *is* the flagged signal (docs/phase0-plan.md
/// §4.3) -- and refreshes the tooltip and the two label-only menu items.
pub fn update(app: &AppHandle, state: Option<&NowPlayingChanged>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let (track_text, status_text, tooltip, flagged) = summarize(state);

    let _ = tray.set_tooltip(Some(tooltip.as_str()));
    let icon_bytes = if flagged { FLAGGED_ICON } else { NEUTRAL_ICON };
    if let Ok(image) = Image::from_bytes(icon_bytes) {
        let _ = tray.set_icon(Some(image));
    }

    if let Some(items) = app.try_state::<TrayMenuItems>() {
        let _ = items.current_track.set_text(track_text);
        let _ = items.flagged_status.set_text(status_text);
    }
}

fn summarize(state: Option<&NowPlayingChanged>) -> (String, String, String, bool) {
    let Some(state) = state else {
        return (
            "Nothing playing".to_string(),
            String::new(),
            "Now Playing — AI-Artist Flagger: idle".to_string(),
            false,
        );
    };

    let names = state
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    // Multi-artist tracks: flagged if any credited artist is flagged.
    let flagged = state.artists.iter().any(|a| a.is_flagged == Some(true));
    let pending = state.artists.iter().any(|a| a.is_flagged.is_none());
    let status_text = if pending {
        "Classifying…".to_string()
    } else if flagged {
        "Flagged: possible AI artist".to_string()
    } else {
        "Not flagged".to_string()
    };
    let track_text = format!("{} — {}", state.track_title, names);
    let tooltip = format!("{track_text}\n{status_text}");

    (track_text, status_text, tooltip, flagged)
}
