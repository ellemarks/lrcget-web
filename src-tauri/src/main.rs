#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use lrcget_lib::commands::{self, Ctx, EventSink};
use lrcget_lib::persistent_entities::PlayableTrack;
use lrcget_lib::player::Player;
use lrcget_lib::state::{AppState, Notify, NotifyType, ServiceAccess};
use lrcget_lib::{db, for_each_command, scanner};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

/// Forwards backend events to the webview.
struct TauriEvents(AppHandle);

impl EventSink for TauriEvents {
    fn emit_value(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }
}

/// Desktop-only state: the native audio player.
struct DesktopState {
    player: Mutex<Option<Player>>,
}

/// Generates a `#[tauri::command]` wrapper for every shared command in `lrcget_lib::commands`.
macro_rules! tauri_command_wrappers {
    ($( $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => {
        $(
            #[tauri::command]
            async fn $name(ctx: State<'_, Ctx>, $($arg: $ty),*) -> $ret {
                commands::$name(&ctx, $($arg),*).await
            }
        )*
    };
}

for_each_command!(tauri_command_wrappers);

/// Builds the invoke handler from the shared command list plus the desktop-only commands.
macro_rules! tauri_invoke_handler {
    ($( $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty; )*) => {
        tauri::generate_handler![
            $($name,)*
            play_track,
            pause_track,
            resume_track,
            seek_track,
            stop_track,
            set_volume,
            set_playback_speed,
            open_devtools,
            drain_notifications,
        ]
    };
}

#[tauri::command]
async fn play_track(
    track_id: Option<i64>,
    file_path: Option<String>,
    title: Option<String>,
    album_name: Option<String>,
    artist_name: Option<String>,
    album_artist_name: Option<String>,
    duration: Option<f64>,
    ctx: State<'_, Ctx>,
    desktop_state: State<'_, DesktopState>,
) -> Result<(), String> {
    let playable_track: PlayableTrack = if let Some(id) = track_id {
        // Database track - fetch and convert
        let db_track = ctx
            .db(|db| db::get_track_by_id(id, db))
            .map_err(|err| err.to_string())?;
        PlayableTrack::from(db_track)
    } else if let Some(path) = file_path {
        // File-based track - create from metadata
        let path_obj = std::path::Path::new(&path);
        let file_name = path_obj
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();

        // Extract metadata from file if not provided
        let (resolved_title, resolved_duration) = if title.is_none() || duration.is_none() {
            match scanner::metadata::TrackMetadata::from_path(path_obj) {
                Ok(metadata) => (
                    title.unwrap_or(metadata.title),
                    duration.unwrap_or(metadata.duration),
                ),
                Err(_) => (
                    title.unwrap_or_else(|| file_name.clone()),
                    duration.unwrap_or(0.0),
                ),
            }
        } else {
            (title.unwrap(), duration.unwrap())
        };

        PlayableTrack {
            id: None,
            file_path: path,
            file_name,
            title: resolved_title,
            album_name: album_name.unwrap_or_default(),
            artist_name: artist_name.unwrap_or_default(),
            album_artist_name,
            image_path: None,
            track_number: None,
            duration: resolved_duration,
            instrumental: false,
            lyricsfile: None,
            lyricsfile_id: None,
        }
    } else {
        return Err("Either track_id or file_path must be provided".to_string());
    };

    let mut player_guard = desktop_state.player.lock().unwrap();
    if let Some(ref mut player) = *player_guard {
        player.play(playable_track).map_err(|err| err.to_string())?;
    }

    Ok(())
}

#[tauri::command]
fn pause_track(desktop_state: State<DesktopState>) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.pause();
    }

    Ok(())
}

#[tauri::command]
fn resume_track(desktop_state: State<DesktopState>) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.resume();
    }

    Ok(())
}

#[tauri::command]
fn seek_track(position: f64, desktop_state: State<DesktopState>) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.seek(position);
    }

    Ok(())
}

#[tauri::command]
fn stop_track(desktop_state: State<DesktopState>) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.stop();
    }

    Ok(())
}

#[tauri::command]
fn set_volume(
    volume: f64,
    ctx: State<Ctx>,
    desktop_state: State<DesktopState>,
) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.set_volume(volume);
    }
    drop(player_guard);

    // Persist volume to config
    commands::set_volume_config(&ctx, volume)
}

#[tauri::command]
fn set_playback_speed(
    playback_speed: f64,
    desktop_state: State<DesktopState>,
) -> Result<(), String> {
    let mut player_guard = desktop_state.player.lock().map_err(|e| e.to_string())?;

    if let Some(ref mut player) = *player_guard {
        player.set_playback_speed(playback_speed);
    }

    Ok(())
}

#[tauri::command]
fn open_devtools(app_handle: AppHandle) {
    app_handle
        .get_webview_window("main")
        .unwrap()
        .open_devtools();
}

#[tauri::command]
fn drain_notifications(ctx: State<Ctx>) -> Vec<Notify> {
    commands::drain_notifications(&ctx)
}

#[tokio::main]
async fn main() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(DesktopState {
            player: Default::default(),
        });

    // W3C WebDriver server (`tauri-plugin-webdriver`) — debug builds only.
    // Listens on http://127.0.0.1:4445 (override with TAURI_WEBDRIVER_PORT);
    // see AGENTS.md for the agent inspection runbook.
    #[cfg(debug_assertions)]
    let builder = builder.plugin(tauri_plugin_webdriver::init());

    builder
        .invoke_handler(for_each_command!(tauri_invoke_handler))
        .setup(|app| {
            let handle = app.handle();

            let app_dir = handle
                .path()
                .app_data_dir()
                .expect("The app data directory should exist.");
            let db = db::initialize_database(&app_dir).expect("Database initialize should succeed");
            let ctx = Ctx::new(
                Arc::new(AppState::new(db)),
                Arc::new(TauriEvents(handle.clone())),
            );

            // Load config to get initial volume
            let initial_volume = ctx
                .db(|db| db::get_config(db))
                .map(|config| config.volume)
                .unwrap_or(1.0);

            let desktop_state: State<DesktopState> = handle.state();
            match Player::new(initial_volume) {
                Ok(player) => {
                    *desktop_state.player.lock().unwrap() = Some(player);
                }
                Err(e) => {
                    eprintln!("Failed to initialize audio player: {}", e);
                    ctx.notify(
                        format!("Failed to initialize audio player: {}", e),
                        NotifyType::Error,
                    );
                }
            }

            handle.manage(ctx);

            let handle_clone = handle.clone();

            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(40));
                loop {
                    interval.tick().await;
                    {
                        let desktop_state: State<DesktopState> = handle_clone.state();
                        let player_guard = desktop_state.player.lock();

                        match player_guard {
                            Ok(mut player_guard) => {
                                if let Some(ref mut player) = *player_guard {
                                    player.renew_state();

                                    let emit_player_state =
                                        handle_clone.emit("player-state", &player);

                                    if let Err(e) = emit_player_state {
                                        eprintln!("Failed to emit player state: {}", e);
                                    }
                                }
                            }
                            Err(e) => eprintln!("Failed to lock player: {}", e),
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
