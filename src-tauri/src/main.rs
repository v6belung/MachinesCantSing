#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod classifier;
mod commands;
mod db;
mod itunes;
mod media;
mod musicbrainz;
mod text;
mod tray;

use std::sync::Arc;

use tauri::Manager;

use db::Db;
use itunes::ItunesClient;
use media::MediaMonitor;
use musicbrainz::MusicBrainzClient;

pub struct AppState {
    pub db: Arc<Db>,
    pub monitor: Arc<MediaMonitor>,
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let app_handle = app.handle().clone();

            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Arc::new(Db::open(&data_dir.join("now-playing-flagger.sqlite3"))?);
            let itunes = Arc::new(ItunesClient::new()?);
            let musicbrainz = Arc::new(MusicBrainzClient::new()?);

            let monitor = Arc::new(MediaMonitor::new(
                app_handle.clone(),
                db.clone(),
                itunes.clone(),
                musicbrainz.clone(),
            ));
            monitor.clone().start();

            tray::setup(&app_handle)?;

            app.manage(AppState { db, monitor });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_current_state,
            commands::get_recent_classifications,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
