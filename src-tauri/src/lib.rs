pub mod commands;
pub mod db;
pub mod error;
pub mod services;
pub mod state;

use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Mutex, RwLock};
use tauri::{
    http::{header, Response, StatusCode},
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::commands::{capture_cmds, clipboard_cmds, gallery_cmds, settings_cmds};
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let state: tauri::State<'_, AppState> = app.state();

                    let alt_s = Shortcut::new(Some(Modifiers::ALT), Code::KeyS);
                    let alt_shift_s =
                        Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyS);
                    let alt_g = Shortcut::new(Some(Modifiers::ALT), Code::KeyG);
                    let alt_v = Shortcut::new(Some(Modifiers::ALT), Code::KeyV);
                    let alt_c = Shortcut::new(Some(Modifiers::ALT), Code::KeyC);

                    if shortcut == &alt_s {
                        let _ = capture_cmds::trigger_region_capture(app.clone(), state);
                    } else if shortcut == &alt_shift_s {
                        let _ = capture_cmds::trigger_fullscreen_capture(app.clone(), state);
                    } else if shortcut == &alt_g {
                        if let Some(main_win) = app.get_webview_window("main") {
                            let is_vis = main_win.is_visible().unwrap_or(false);
                            let is_foc = main_win.is_focused().unwrap_or(false);
                            if is_vis && is_foc {
                                let _ = main_win.emit(
                                    "shortcut:toast",
                                    "Alt + G pressed — Gallery is already active (Close window to minimize to tray)",
                                );
                            } else {
                                let _ = main_win.show();
                                let _ = main_win.unminimize();
                                let _ = main_win.set_focus();
                                let _ = main_win.emit("shortcut:toast", "Opened Gallery via Alt + G");
                            }
                        }
                    } else if shortcut == &alt_v {
                        let _ = clipboard_cmds::toggle_clipboard_drawer(app.clone());
                    } else if shortcut == &alt_c {
                        if let Some(main_win) = app.get_webview_window("main") {
                            let _ = main_win.show();
                            let _ = main_win.set_focus();
                            let _ = main_win.emit(
                                "shortcut:toast",
                                "Alt + C Save Replay Clip is scheduled for Phase 2.0!",
                            );
                        }
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
            // Default user-visible captures directory in Pictures/ReplayViewer (fallback to AppData/Local)
            let base_media_dir = app
                .path()
                .picture_dir()
                .map(|p| p.join("ReplayViewer"))
                .or_else(|_| app.path().app_local_data_dir())
                .expect("Failed to resolve media directory");

            // Store SQLite DB in AppData/Local/com.replayviewer.app (next to EBWebView)
            let local_data_dir = app
                .path()
                .app_local_data_dir()
                .unwrap_or_else(|_| base_media_dir.clone());
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
                (
                    base_media_dir.join("captures"),
                    base_media_dir.join("thumbnails"),
                )
            };

            std::fs::create_dir_all(&captures_dir)?;
            std::fs::create_dir_all(&thumbs_dir)?;

            let clipboard_cache_dir = local_data_dir.join("clipboard_cache");
            std::fs::create_dir_all(&clipboard_cache_dir)?;

            // Phase 1.0 Guarantee #4: Reconcile any orphaned DB rows on startup
            let _ = db::captures_repo::reconcile_orphaned_captures(&conn);

            app.manage(AppState {
                db: Mutex::new(conn),
                latest_capture: RwLock::new(None),
                captures_dir: RwLock::new(captures_dir),
                thumbs_dir: RwLock::new(thumbs_dir),
                clipboard_cache_dir,
                clipboard_paused: AtomicBool::new(false),
            });

            // Phase 1.5.1: Spawn background OS clipboard image watcher
            services::clipboard::spawn_clipboard_watcher(app.handle().clone());

            // Register Global Shortcuts (Alt+S, Alt+Shift+S, Alt+G, Alt+V, Alt+C)
            let alt_s = Shortcut::new(Some(Modifiers::ALT), Code::KeyS);
            let alt_shift_s = Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyS);
            let alt_g = Shortcut::new(Some(Modifiers::ALT), Code::KeyG);
            let alt_v = Shortcut::new(Some(Modifiers::ALT), Code::KeyV);
            let alt_c = Shortcut::new(Some(Modifiers::ALT), Code::KeyC);
            let _ = app.global_shortcut().register(alt_s);
            let _ = app.global_shortcut().register(alt_shift_s);
            let _ = app.global_shortcut().register(alt_g);
            let _ = app.global_shortcut().register(alt_v);
            let _ = app.global_shortcut().register(alt_c);

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
                    &open_gallery_i,
                    &quit_i,
                ],
            )?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("ReplayViewer — Ready (Alt+S | Alt+V)")
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
                    "open_gallery" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
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
            gallery_cmds::get_latest_capture_meta,
            gallery_cmds::add_tag_to_capture,
            settings_cmds::get_settings,
            settings_cmds::update_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
