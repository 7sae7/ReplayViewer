pub mod commands;
pub mod db;
pub mod error;
pub mod services;
pub mod single_instance;
pub mod state;

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex, RwLock,
};
use tauri::{
    http::{header, Response, StatusCode},
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::commands::{capture_cmds, clipboard_cmds, gallery_cmds, replay_cmds, settings_cmds};
use crate::state::{ActiveShortcuts, AppState, ReplayRuntimeState};

/// Parses a user-configured shortcut string (e.g. `"Alt+S"`, `"Ctrl+Shift+X"`, `"F9"`, or `""` when unbound)
/// into a `tauri_plugin_global_shortcut::Shortcut`.
pub fn parse_user_shortcut(combo: &str) -> Option<Shortcut> {
    let trimmed = combo.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("unbound")
    {
        return None;
    }

    let normalized = trimmed
        .split('+')
        .map(|part| match part.trim() {
            " " => "Space".to_string(),
            "ArrowUp" => "Up".to_string(),
            "ArrowDown" => "Down".to_string(),
            "ArrowLeft" => "Left".to_string(),
            "ArrowRight" => "Right".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("+");

    Shortcut::from_str(&normalized).ok()
}

/// Reads the user's configured hotkeys from SQLite `settings`, unregisters all previous OS global shortcuts,
/// registers only the active (non-empty) shortcuts, and updates `state.active_shortcuts`.
pub fn sync_global_shortcuts(app: &tauri::AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let _ = app.global_shortcut().unregister_all();

    let Ok(cfg) = settings_cmds::get_settings(state.clone()) else {
        return;
    };

    let active = ActiveShortcuts {
        region_capture: parse_user_shortcut(&cfg.hotkey_region_capture),
        fullscreen_capture: parse_user_shortcut(&cfg.hotkey_fullscreen_capture),
        open_gallery: parse_user_shortcut(&cfg.hotkey_open_gallery),
        visual_clipboard: parse_user_shortcut(&cfg.hotkey_visual_clipboard),
        save_replay: parse_user_shortcut(&cfg.hotkey_save_replay),
    };

    for sc in [
        active.region_capture,
        active.fullscreen_capture,
        active.open_gallery,
        active.visual_clipboard,
        active.save_replay,
    ]
    .into_iter()
    .flatten()
    {
        let _ = app.global_shortcut().register(sc);
    }

    if let Ok(mut guard) = state.active_shortcuts.write() {
        *guard = active;
    };
}

/// Resolves the built-in portable `captures` directory right next to the running `.exe`
/// (`<exe_dir>/captures` and `<exe_dir>/captures/.thumbs`), falling back to `AppData/Local`
/// only if the `.exe` directory is read-only (e.g., installed inside `C:\Program Files`).
pub fn resolve_default_captures_dir(app: &tauri::AppHandle) -> (PathBuf, PathBuf) {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let portable_captures = exe_dir.join("captures");
            let portable_thumbs = portable_captures.join(".thumbs");
            if std::fs::create_dir_all(&portable_captures).is_ok()
                && std::fs::create_dir_all(&portable_thumbs).is_ok()
            {
                return (portable_captures, portable_thumbs);
            }
        }
    }

    let fallback_base = app
        .path()
        .app_local_data_dir()
        .or_else(|_| app.path().picture_dir().map(|p| p.join("ReplayViewer")))
        .expect("Failed to resolve fallback media directory");
    let captures = fallback_base.join("captures");
    let thumbs = captures.join(".thumbs");
    (captures, thumbs)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Enforce a single running instance: if another ReplayViewer instance is already open,
    // signal it to show & focus the main Gallery window and exit immediately (no duplicate tray icon).
    let Some(single_instance_listener) = single_instance::acquire_single_instance_listener() else {
        return;
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let state: tauri::State<'_, AppState> = app.state();

                    // Ignore global shortcuts if user is currently recording a keybind in Settings
                    if state.shortcut_recording.load(Ordering::Relaxed) {
                        return;
                    }

                    let active = state
                        .active_shortcuts
                        .read()
                        .map(|g| g.clone())
                        .unwrap_or_default();

                    if Some(*shortcut) == active.region_capture {
                        let _ = capture_cmds::trigger_region_capture(app.clone(), state);
                    } else if Some(*shortcut) == active.fullscreen_capture {
                        let _ = capture_cmds::trigger_fullscreen_capture(app.clone(), state);
                    } else if Some(*shortcut) == active.open_gallery {
                        if let Some(main_win) = app.get_webview_window("main") {
                            let is_vis = main_win.is_visible().unwrap_or(false);
                            let is_min = main_win.is_minimized().unwrap_or(false);
                            if is_vis && !is_min {
                                let _ = main_win.hide();
                            } else {
                                let _ = main_win.show();
                                let _ = main_win.unminimize();
                                let _ = main_win.set_focus();
                            }
                        }
                    } else if Some(*shortcut) == active.visual_clipboard {
                        let _ = clipboard_cmds::toggle_clipboard_drawer(app.clone());
                    } else if Some(*shortcut) == active.save_replay {
                        replay_cmds::spawn_save_replay_in_background(app.clone());
                    }
                })
                .build(),
        )
        // Custom `capture` binary protocol (`http://capture.localhost/current` on Windows,
        // `capture://localhost/current` on macOS/Linux) for zero-Base64 uncompressed BMP frame streaming
        .register_uri_scheme_protocol("capture", |ctx, _request| {
            let state: tauri::State<'_, AppState> = ctx.app_handle().state();
            if let Ok(guard) = state.latest_capture.read() {
                if let Some(ref frame) = *guard {
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "image/bmp")
                        .header(header::CACHE_CONTROL, "no-store")
                        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                        .body(frame.bmp_bytes.as_ref().clone())
                        .unwrap();
                }
            }
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Vec::new())
                .unwrap()
        })
        .setup(|app| {
            // Default portable captures directory right next to the .exe (<exe_dir>/captures)
            let (default_captures_dir, default_thumbs_dir) =
                resolve_default_captures_dir(app.handle());

            // Store SQLite DB in AppData/Local/com.replayviewer.app (next to EBWebView)
            let local_data_dir = app
                .path()
                .app_local_data_dir()
                .unwrap_or_else(|_| default_captures_dir.clone());
            std::fs::create_dir_all(&local_data_dir)?;
            let db_path = local_data_dir.join("replay_viewer_v2.db");
            let conn = db::init_db(&db_path)?;

            // Check if user configured a custom Storage Directory in Settings
            let configured_dir: Option<String> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'captures_dir'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|v| serde_json::from_str::<String>(&v).ok())
                .filter(|s| !s.trim().is_empty());

            let (captures_dir, thumbs_dir) = if let Some(custom) = configured_dir {
                let c_dir = PathBuf::from(custom);
                let t_dir = c_dir.join(".thumbs");
                (c_dir, t_dir)
            } else {
                (default_captures_dir, default_thumbs_dir)
            };

            std::fs::create_dir_all(&captures_dir)?;
            std::fs::create_dir_all(&thumbs_dir)?;

            let clipboard_cache_dir = local_data_dir.join("clipboard_cache");
            std::fs::create_dir_all(&clipboard_cache_dir)?;

            let replay_bin_dir = local_data_dir.join("bin");
            let replay_ring_dir = local_data_dir.join("replay_ring");
            std::fs::create_dir_all(&replay_bin_dir)?;
            std::fs::create_dir_all(&replay_ring_dir)?;

            // Check initial replay_enabled & audio gain preferences cleanly
            let initial_replay_enabled: bool = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'replay_enabled'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|v| serde_json::from_str::<bool>(&v).ok())
                .unwrap_or(true);

            let initial_desktop_gain: f32 = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'audio_desktop_volume'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|v| serde_json::from_str::<f32>(&v).ok())
                .unwrap_or(1.0)
                .clamp(0.0, 2.0);

            let initial_mic_gain: f32 = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'audio_mic_volume'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|v| serde_json::from_str::<f32>(&v).ok())
                .unwrap_or(1.0)
                .clamp(0.0, 2.0);

            let initial_app_gains: HashMap<String, f32> = conn
                .query_row(
                    "SELECT value FROM settings WHERE key = 'audio_app_volumes'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .and_then(|v| serde_json::from_str::<HashMap<String, f32>>(&v).ok())
                .unwrap_or_default();

            // Phase 1.0 Guarantee #4: Reconcile any orphaned DB rows on startup
            let _ = db::captures_repo::reconcile_orphaned_captures(&conn);

            app.manage(AppState {
                db: Mutex::new(conn),
                latest_capture: RwLock::new(None),
                captures_dir: RwLock::new(captures_dir),
                thumbs_dir: RwLock::new(thumbs_dir),
                clipboard_cache_dir,
                replay_bin_dir,
                replay_ring_dir,
                clipboard_paused: AtomicBool::new(false),
                active_clipboard_hash: RwLock::new(None),
                active_shortcuts: RwLock::new(ActiveShortcuts::default()),
                shortcut_recording: AtomicBool::new(false),
                replay_should_run: AtomicBool::new(initial_replay_enabled),
                replay_restart_requested: AtomicBool::new(false),
                replay_child_pid: AtomicU32::new(0),
                replay_runtime: RwLock::new(ReplayRuntimeState::default()),
                cached_ffmpeg_status: RwLock::new(None),
                ffmpeg_downloading: AtomicBool::new(false),
                ffmpeg_download_stage: RwLock::new(String::new()),
                mic_test_running: Arc::new(AtomicBool::new(false)),
                mic_test_sidetone: Arc::new(AtomicBool::new(false)),
                mic_test_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
                mic_test_gain_bits: Arc::new(AtomicU32::new(initial_mic_gain.to_bits())),
                audio_desktop_gain_bits: Arc::new(AtomicU32::new(initial_desktop_gain.to_bits())),
                audio_mic_gain_bits: Arc::new(AtomicU32::new(initial_mic_gain.to_bits())),
                audio_app_gains: Arc::new(RwLock::new(initial_app_gains)),
                audio_route_gen: Arc::new(std::sync::atomic::AtomicU64::new(1)),
                audio_route_plan: Arc::new(RwLock::new(
                    crate::services::audio::LiveAudioRoutePlan::default(),
                )),
                replay_stream_slots: RwLock::new(Vec::new()),
                audio_last_active_ms: Mutex::new(HashMap::new()),
                slot_had_audio: Arc::new([
                    AtomicBool::new(false),
                    AtomicBool::new(false),
                    AtomicBool::new(false),
                    AtomicBool::new(false),
                ]),
            });

            // Listen for subsequent `.exe` launches and bring the existing `main` window to the foreground
            single_instance::spawn_single_instance_focus_handler(
                single_instance_listener,
                app.handle().clone(),
            );

            // Phase 1.5.1: Spawn background OS clipboard image watcher
            services::clipboard::spawn_clipboard_watcher(app.handle().clone());

            // Phase 2.0.1 & 2.0.2: Spawn background segmented hardware replay buffer & dynamic WASAPI audio supervisor
            services::replay::spawn_replay_supervisor(app.handle().clone());

            // Phase 1.5.3 & 1.5.4: Enforce 3-day storage grace quota & backfill pending native OS OCR
            let _ = services::storage_cleanup::enforce_storage_policies_now(app.handle());
            let _ = services::ocr::run_ocr_backfill_batch(app.handle(), 50);

            // Dynamically register user-configured Global Shortcuts from SQLite `settings`
            sync_global_shortcuts(app.handle());

            // System Tray Setup
            let capture_region_i = MenuItem::with_id(
                app,
                "capture_region",
                "Capture Region (Alt + S)",
                true,
                None::<&str>,
            )?;
            let capture_full_i = MenuItem::with_id(
                app,
                "capture_fullscreen",
                "Instant Fullscreen (Alt + Shift + S)",
                true,
                None::<&str>,
            )?;
            let open_clipboard_i = MenuItem::with_id(
                app,
                "open_clipboard",
                "Visual Clipboard (Alt + V)",
                true,
                None::<&str>,
            )?;
            let save_replay_i = MenuItem::with_id(
                app,
                "save_replay",
                "Save Instant Replay (Alt + C)",
                true,
                None::<&str>,
            )?;
            let toggle_replay_i = MenuItem::with_id(
                app,
                "toggle_replay_buffer",
                "Start / Pause Replay Buffer",
                true,
                None::<&str>,
            )?;
            let toggle_pause_clipboard_i = MenuItem::with_id(
                app,
                "toggle_pause_clipboard",
                "Pause / Resume Clipboard History",
                true,
                None::<&str>,
            )?;
            let open_gallery_i = MenuItem::with_id(
                app,
                "open_gallery",
                "Open Gallery (Alt + G)",
                true,
                None::<&str>,
            )?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit ReplayViewer", true, None::<&str>)?;

            let menu = Menu::with_items(
                app,
                &[
                    &capture_region_i,
                    &capture_full_i,
                    &open_clipboard_i,
                    &save_replay_i,
                    &toggle_replay_i,
                    &toggle_pause_clipboard_i,
                    &open_gallery_i,
                    &quit_i,
                ],
            )?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("ReplayViewer — Ready (Alt+S | Alt+V | Alt+C)")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "capture_region" => {
                        let state: tauri::State<'_, AppState> = app.state();
                        let _ = capture_cmds::trigger_region_capture(app.clone(), state);
                    }
                    "capture_fullscreen" => {
                        let state: tauri::State<'_, AppState> = app.state();
                        let _ = capture_cmds::trigger_fullscreen_capture(app.clone(), state);
                    }
                    "open_clipboard" => {
                        let _ = clipboard_cmds::toggle_clipboard_drawer(app.clone());
                    }
                    "save_replay" => {
                        replay_cmds::spawn_save_replay_in_background(app.clone());
                    }
                    "toggle_replay_buffer" => {
                        let state: tauri::State<'_, AppState> = app.state();
                        let prev = state.replay_should_run.load(Ordering::Relaxed);
                        let next = !prev;
                        let _ = replay_cmds::set_replay_buffer_active(app.clone(), state, next);
                        let _ = app.emit(
                            "shortcut:toast",
                            if next {
                                "Replay Buffer Started"
                            } else {
                                "Replay Buffer Paused"
                            },
                        );
                    }
                    "toggle_pause_clipboard" => {
                        let state: tauri::State<'_, AppState> = app.state();
                        let prev = state
                            .clipboard_paused
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let next = !prev;
                        state
                            .clipboard_paused
                            .store(next, std::sync::atomic::Ordering::Relaxed);
                        let _ = app.emit("clipboard:paused_changed", next);
                        let _ = app.emit(
                            "shortcut:toast",
                            if next {
                                "Clipboard History Paused (Sensitive copies ignored)"
                            } else {
                                "Clipboard History Resumed"
                            },
                        );
                    }
                    "open_gallery" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
                        let state: tauri::State<'_, AppState> = app.state();
                        state.replay_should_run.store(false, Ordering::SeqCst);
                        state.replay_restart_requested.store(true, Ordering::SeqCst);
                        let pid = state.replay_child_pid.load(Ordering::Relaxed);
                        if pid != 0 {
                            #[cfg(target_os = "windows")]
                            {
                                use std::os::windows::process::CommandExt;
                                let _ = std::process::Command::new("taskkill")
                                    .args(["/F", "/PID", &pid.to_string()])
                                    .creation_flags(0x08000000)
                                    .output();
                            }
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        // Minimize `main`, `overlay`, and `clipboard` windows to System Tray on close instead of terminating background process
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let lbl = window.label();
                if lbl == "main" || lbl == "overlay" || lbl == "clipboard" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            capture_cmds::trigger_region_capture,
            capture_cmds::trigger_fullscreen_capture,
            capture_cmds::save_annotated_capture,
            clipboard_cmds::toggle_clipboard_drawer,
            clipboard_cmds::get_clipboard_always_on_top,
            clipboard_cmds::set_clipboard_always_on_top,
            clipboard_cmds::list_clipboard_history,
            clipboard_cmds::paste_clipboard_item,
            clipboard_cmds::annotate_clipboard_item,
            clipboard_cmds::promote_clipboard_to_gallery,
            clipboard_cmds::toggle_pin_clipboard_item,
            clipboard_cmds::delete_clipboard_item,
            clipboard_cmds::clear_unpinned_clipboard_history,
            clipboard_cmds::get_clipboard_paused,
            clipboard_cmds::set_clipboard_paused,
            gallery_cmds::list_captures,
            gallery_cmds::scan_unindexed_media,
            gallery_cmds::import_external_media,
            gallery_cmds::migrate_captures_to_folder,
            gallery_cmds::toggle_pin_capture,
            gallery_cmds::delete_captures,
            gallery_cmds::batch_add_tags,
            gallery_cmds::batch_set_group,
            gallery_cmds::move_captures_to_folder,
            gallery_cmds::reconcile_orphaned_captures,
            gallery_cmds::copy_capture_to_clipboard,
            gallery_cmds::pick_folder_dialog,
            gallery_cmds::open_in_file_explorer,
            gallery_cmds::seed_benchmark_captures,
            gallery_cmds::restore_queued_captures,
            gallery_cmds::get_storage_status_preview,
            gallery_cmds::enforce_storage_policies_now,
            gallery_cmds::run_ocr_backfill_now,
            gallery_cmds::get_latest_capture_meta,
            gallery_cmds::add_tag_to_capture,
            settings_cmds::get_settings,
            settings_cmds::update_settings,
            settings_cmds::set_shortcut_recording_mode,
            settings_cmds::get_default_captures_dir,
            replay_cmds::get_ffmpeg_status,
            replay_cmds::download_ffmpeg_engine,
            replay_cmds::get_replay_buffer_snapshot,
            replay_cmds::set_replay_buffer_active,
            replay_cmds::save_replay_buffer,
            replay_cmds::probe_video_audio_tracks,
            replay_cmds::prepare_video_track_previews_cmd,
            replay_cmds::cleanup_video_track_previews_cmd,
            replay_cmds::trim_video_clip_cmd,
            replay_cmds::list_active_audio_sessions,
            replay_cmds::get_live_audio_peaks,
            replay_cmds::list_audio_input_devices,
            replay_cmds::start_mic_test,
            replay_cmds::stop_mic_test,
            replay_cmds::set_mic_test_sidetone,
            replay_cmds::set_mic_test_gain,
            replay_cmds::get_mic_test_level,
            replay_cmds::set_audio_track_volumes,
            replay_cmds::pick_ffmpeg_dialog,
            replay_cmds::update_app_audio_rule,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_shortcut_and_unbound() {
        assert!(parse_user_shortcut("Alt+S").is_some());
        assert!(parse_user_shortcut("Alt+Shift+S").is_some());
        assert!(parse_user_shortcut("Ctrl+Shift+X").is_some());
        assert!(parse_user_shortcut("F9").is_some());
        assert!(parse_user_shortcut("").is_none());
        assert!(parse_user_shortcut("None").is_none());
        assert!(parse_user_shortcut("Unbound").is_none());
    }
}
