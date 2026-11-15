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
    pub launch_on_startup: bool,
    pub captures_dir: String,
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

        let pairs: [(&str, String); 12] = [
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

    // Enforce storage quota / retention rules (3-day Queued Deletion grace buffer)
    let _ = crate::services::storage_cleanup::enforce_storage_policies_now(&app);

    Ok(payload)
}

#[tauri::command]
pub fn get_default_captures_dir(app: AppHandle) -> AppResult<String> {
    let (default_captures, _) = crate::resolve_default_captures_dir(&app);
    Ok(default_captures.to_string_lossy().to_string())
}
