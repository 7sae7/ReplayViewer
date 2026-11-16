use std::collections::HashMap;
use std::path::PathBuf;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::db::clipboard_repo::enforce_fifo_limit;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPayload {
    pub hotkey_region_capture: String,
    pub hotkey_fullscreen_capture: String,
    pub hotkey_open_gallery: String,
    pub hotkey_visual_clipboard: String,
    pub hotkey_save_replay: String,
    pub image_format: String,
    pub copy_to_clipboard_on_save: bool,
    pub max_storage_gb: u32,
    pub auto_cleanup_days: u32,
    pub clipboard_max_items: u32,
    #[serde(default)]
    pub clipboard_excluded_apps: Vec<String>,
    #[serde(default = "default_true")]
    pub replay_enabled: bool,
    #[serde(default = "default_replay_buffer_secs")]
    pub replay_buffer_secs: u32,
    #[serde(default = "default_replay_fps")]
    pub replay_fps: u32,
    #[serde(default = "default_replay_bitrate_mbps")]
    pub replay_bitrate_mbps: u32,
    #[serde(default = "default_replay_encoder")]
    pub replay_encoder: String,
    #[serde(default)]
    pub ffmpeg_custom_path: String,
    #[serde(default = "default_true")]
    pub audio_record_desktop: bool,
    #[serde(default = "default_true")]
    pub audio_record_mic: bool,
    #[serde(default = "default_one")]
    pub audio_desktop_volume: f32,
    #[serde(default = "default_one")]
    pub audio_mic_volume: f32,
    #[serde(default)]
    pub audio_app_volumes: HashMap<String, f32>,
    #[serde(default)]
    pub audio_mic_device_id: String,
    #[serde(default)]
    pub audio_single_mix: bool,
    #[serde(default)]
    pub audio_muted_apps: Vec<String>,
    #[serde(default)]
    pub audio_isolated_apps: Vec<String>,
    pub launch_on_startup: bool,
    pub captures_dir: String,
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn default_replay_buffer_secs() -> u32 {
    60
}

fn default_replay_fps() -> u32 {
    60
}

fn default_replay_bitrate_mbps() -> u32 {
    20
}

fn default_replay_encoder() -> String {
    "auto".to_string()
}

pub fn default_excluded_apps() -> Vec<String> {
    vec![
        "1password".to_string(),
        "bitwarden".to_string(),
        "keepass".to_string(),
        "keepassxc".to_string(),
        "dashlane".to_string(),
        "lastpass".to_string(),
        "nordpass".to_string(),
        "protonpass".to_string(),
        "enpass".to_string(),
        "inprivate".to_string(),
        "incognito".to_string(),
        "private browsing".to_string(),
        "tor browser".to_string(),
    ]
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<SettingsPayload> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (k, v) = row?;
        map.insert(k, v);
    }

    let get_str = |k: &str, default: &str| -> String {
        map.get(k)
            .and_then(|v| serde_json::from_str::<String>(v).ok())
            .unwrap_or_else(|| default.to_string())
    };

    let get_u32 = |k: &str, default: u32| -> u32 {
        map.get(k)
            .and_then(|v| serde_json::from_str::<u32>(v).ok())
            .unwrap_or(default)
    };

    let get_bool = |k: &str, default: bool| -> bool {
        map.get(k)
            .and_then(|v| serde_json::from_str::<bool>(v).ok())
            .unwrap_or(default)
    };

    let get_f32 = |k: &str, default: f32| -> f32 {
        map.get(k)
            .and_then(|v| serde_json::from_str::<f32>(v).ok())
            .unwrap_or(default)
    };

    let audio_app_volumes: HashMap<String, f32> = map
        .get("audio_app_volumes")
        .and_then(|v| serde_json::from_str::<HashMap<String, f32>>(v).ok())
        .unwrap_or_default();

    let excluded_apps: Vec<String> = map
        .get("clipboard_excluded_apps")
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .unwrap_or_else(default_excluded_apps);

    let audio_muted_apps: Vec<String> = map
        .get("audio_muted_apps")
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .unwrap_or_default();

    let audio_isolated_apps: Vec<String> = map
        .get("audio_isolated_apps")
        .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
        .unwrap_or_default();

    let raw_replay_key = get_str("hotkey_save_replay", "Alt+C");
    let replay_key = if raw_replay_key == "Alt+R" {
        "Alt+C".to_string()
    } else {
        raw_replay_key
    };

    let current_dir = state
        .captures_dir
        .read()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();

    Ok(SettingsPayload {
        hotkey_region_capture: get_str("hotkey_region_capture", "Alt+S"),
        hotkey_fullscreen_capture: get_str("hotkey_fullscreen_capture", "Alt+Shift+S"),
        hotkey_open_gallery: get_str("hotkey_open_gallery", "Alt+G"),
        hotkey_visual_clipboard: get_str("hotkey_visual_clipboard", "Alt+V"),
        hotkey_save_replay: replay_key,
        image_format: get_str("image_format", "webp"),
        copy_to_clipboard_on_save: get_bool("copy_to_clipboard_on_save", true),
        max_storage_gb: get_u32("max_storage_gb", 10),
        auto_cleanup_days: get_u32("auto_cleanup_days", 0),
        clipboard_max_items: get_u32("clipboard_max_items", 10),
        clipboard_excluded_apps: excluded_apps,
        replay_enabled: get_bool("replay_enabled", true),
        replay_buffer_secs: get_u32("replay_buffer_secs", 60),
        replay_fps: get_u32("replay_fps", 60),
        replay_bitrate_mbps: get_u32("replay_bitrate_mbps", 20),
        replay_encoder: get_str("replay_encoder", "auto"),
        ffmpeg_custom_path: get_str("ffmpeg_custom_path", ""),
        audio_record_desktop: get_bool("audio_record_desktop", true),
        audio_record_mic: get_bool("audio_record_mic", true),
        audio_desktop_volume: get_f32("audio_desktop_volume", 1.0).clamp(0.0, 2.0),
        audio_mic_volume: get_f32("audio_mic_volume", 1.0).clamp(0.0, 2.0),
        audio_app_volumes,
        audio_mic_device_id: get_str("audio_mic_device_id", ""),
        audio_single_mix: get_bool("audio_single_mix", false),
        audio_muted_apps,
        audio_isolated_apps,
        launch_on_startup: get_bool("launch_on_startup", false),
        captures_dir: get_str("captures_dir", &current_dir),
    })
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: SettingsPayload,
) -> AppResult<SettingsPayload> {
    let prev_cfg = get_settings(state.clone()).ok();

    let trimmed_dir = payload.captures_dir.trim();
    if !trimmed_dir.is_empty() {
        let new_captures_dir = PathBuf::from(trimmed_dir);
        let new_thumbs_dir = new_captures_dir.join(".thumbs");
        std::fs::create_dir_all(&new_captures_dir)?;
        std::fs::create_dir_all(&new_thumbs_dir)?;

        if let Ok(mut cap_guard) = state.captures_dir.write() {
            *cap_guard = new_captures_dir;
        }
        if let Ok(mut thumb_guard) = state.thumbs_dir.write() {
            *thumb_guard = new_thumbs_dir;
        }
    }

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

        let pairs: [(&str, String); 28] = [
            ("hotkey_region_capture", serde_json::to_string(&payload.hotkey_region_capture)?),
            ("hotkey_fullscreen_capture", serde_json::to_string(&payload.hotkey_fullscreen_capture)?),
            ("hotkey_open_gallery", serde_json::to_string(&payload.hotkey_open_gallery)?),
            ("hotkey_visual_clipboard", serde_json::to_string(&payload.hotkey_visual_clipboard)?),
            ("hotkey_save_replay", serde_json::to_string(&payload.hotkey_save_replay)?),
            ("image_format", serde_json::to_string(&payload.image_format)?),
            ("copy_to_clipboard_on_save", serde_json::to_string(&payload.copy_to_clipboard_on_save)?),
            ("max_storage_gb", serde_json::to_string(&payload.max_storage_gb)?),
            ("auto_cleanup_days", serde_json::to_string(&payload.auto_cleanup_days)?),
            ("clipboard_max_items", serde_json::to_string(&payload.clipboard_max_items)?),
            ("clipboard_excluded_apps", serde_json::to_string(&payload.clipboard_excluded_apps)?),
            ("replay_enabled", serde_json::to_string(&payload.replay_enabled)?),
            ("replay_buffer_secs", serde_json::to_string(&payload.replay_buffer_secs)?),
            ("replay_fps", serde_json::to_string(&payload.replay_fps)?),
            ("replay_bitrate_mbps", serde_json::to_string(&payload.replay_bitrate_mbps)?),
            ("replay_encoder", serde_json::to_string(&payload.replay_encoder)?),
            ("ffmpeg_custom_path", serde_json::to_string(&payload.ffmpeg_custom_path)?),
            ("audio_record_desktop", serde_json::to_string(&payload.audio_record_desktop)?),
            ("audio_record_mic", serde_json::to_string(&payload.audio_record_mic)?),
            ("audio_desktop_volume", serde_json::to_string(&payload.audio_desktop_volume)?),
            ("audio_mic_volume", serde_json::to_string(&payload.audio_mic_volume)?),
            ("audio_app_volumes", serde_json::to_string(&payload.audio_app_volumes)?),
            ("audio_mic_device_id", serde_json::to_string(&payload.audio_mic_device_id)?),
            ("audio_single_mix", serde_json::to_string(&payload.audio_single_mix)?),
            ("audio_muted_apps", serde_json::to_string(&payload.audio_muted_apps)?),
            ("audio_isolated_apps", serde_json::to_string(&payload.audio_isolated_apps)?),
            ("launch_on_startup", serde_json::to_string(&payload.launch_on_startup)?),
            ("captures_dir", serde_json::to_string(&payload.captures_dir)?),
        ];

        for (k, v) in pairs {
            conn.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![k, v],
            )?;
        }

        // Immediately enforce new FIFO limit on unpinned clipboard items if lowered
        let _ = enforce_fifo_limit(&conn, payload.clipboard_max_items);
    }

    // Always update live lock-free audio gain values in memory (zero FFmpeg restart)
    state.audio_desktop_gain_bits.store(
        payload.audio_desktop_volume.clamp(0.0, 2.0).to_bits(),
        std::sync::atomic::Ordering::Relaxed,
    );
    state.audio_mic_gain_bits.store(
        payload.audio_mic_volume.clamp(0.0, 2.0).to_bits(),
        std::sync::atomic::Ordering::Relaxed,
    );
    state.mic_test_gain_bits.store(
        payload.audio_mic_volume.clamp(0.0, 2.0).to_bits(),
        std::sync::atomic::Ordering::Relaxed,
    );
    if let Ok(mut g) = state.audio_app_gains.write() {
        *g = payload
            .audio_app_volumes
            .iter()
            .map(|(k, &v)| (k.trim().to_lowercase(), v.clamp(0.0, 2.0)))
            .collect();
    }

    // Re-register OS global shortcuts immediately so any changed, reset, or removed keybind is active!
    crate::sync_global_shortcuts(&app);

    // Only invalidate cached FFmpeg status & restart FFmpeg if structural capture/routing settings changed
    let ffmpeg_path_changed = prev_cfg
        .as_ref()
        .map(|p| p.ffmpeg_custom_path != payload.ffmpeg_custom_path)
        .unwrap_or(true);
    if ffmpeg_path_changed {
        if let Ok(mut guard) = state.cached_ffmpeg_status.write() {
            *guard = None;
        }
    }

    // Hot-swap Mute / Isolate route plan without restarting FFmpeg!
    crate::services::audio::refresh_live_audio_route_plan(
        &app,
        &payload.audio_muted_apps,
        &payload.audio_isolated_apps,
    );

    let structural_replay_changed = prev_cfg
        .as_ref()
        .map(|p| {
            p.replay_enabled != payload.replay_enabled
                || p.replay_buffer_secs != payload.replay_buffer_secs
                || p.replay_fps != payload.replay_fps
                || p.replay_bitrate_mbps != payload.replay_bitrate_mbps
                || p.replay_encoder != payload.replay_encoder
                || p.ffmpeg_custom_path != payload.ffmpeg_custom_path
                || p.audio_record_desktop != payload.audio_record_desktop
                || p.audio_record_mic != payload.audio_record_mic
                || p.audio_mic_device_id != payload.audio_mic_device_id
                || p.audio_single_mix != payload.audio_single_mix
        })
        .unwrap_or(true);

    state
        .replay_should_run
        .store(payload.replay_enabled, std::sync::atomic::Ordering::SeqCst);
    if structural_replay_changed {
        state
            .replay_restart_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    // Enforce storage quota / retention rules (3-day Queued Deletion grace buffer)
    let _ = crate::services::storage_cleanup::enforce_storage_policies_now(&app);

    Ok(payload)
}

/// Pauses or resumes OS global shortcuts while the user is actively recording a keybind in Settings.
/// Unregistering OS shortcuts during recording prevents pressing `Alt+S` (or any other bound keybind)
/// from firing a screenshot instead of being captured by the keybind recorder!
#[tauri::command]
pub fn set_shortcut_recording_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    recording: bool,
) -> AppResult<()> {
    use std::sync::atomic::Ordering;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    state.shortcut_recording.store(recording, Ordering::SeqCst);
    if recording {
        let _ = app.global_shortcut().unregister_all();
    } else {
        crate::sync_global_shortcuts(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn get_default_captures_dir(app: AppHandle) -> AppResult<String> {
    let (default_captures, _) = crate::resolve_default_captures_dir(&app);
    Ok(default_captures.to_string_lossy().to_string())
}
