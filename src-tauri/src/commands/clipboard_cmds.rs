use std::io::Cursor;
use std::sync::{atomic::Ordering, Arc};
use chrono::Utc;
use image::{DynamicImage, ImageFormat};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use uuid::Uuid;

use crate::db::captures_repo::{insert_capture, CaptureRecord};
use crate::db::clipboard_repo::{
    self, bump_clipboard_item_timestamp, get_clipboard_always_on_top as repo_get_always_on_top,
    get_clipboard_item, set_clipboard_always_on_top as repo_set_always_on_top, ClipboardItem,
};
use crate::error::{AppError, AppResult};
use crate::services::capture::persist_raw_rgba_and_thumbnail;
use crate::services::clipboard::resolve_editable_clipboard_image_path;
use crate::state::{AppState, CaptureSessionMeta, FrozenFrame};

/// Toggles the frameless `clipboard` drawer popup window (`Alt + V`),
/// respecting the user's persisted `clipboard_always_on_top` preference.
#[tauri::command]
pub fn toggle_clipboard_drawer(app: AppHandle) -> AppResult<bool> {
    if let Some(win) = app.get_webview_window("clipboard") {
        let is_vis = win.is_visible().unwrap_or(false);
        if is_vis {
            let _ = win.hide();
            return Ok(false);
        } else {
            let state: State<'_, AppState> = app.state();
            let always_on_top = state
                .db
                .lock()
                .map(|conn| repo_get_always_on_top(&conn))
                .unwrap_or(true);

            let _ = win.set_always_on_top(always_on_top);
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
            let _ = win.emit("clipboard:opened", always_on_top);
            return Ok(true);
        }
    }
    Ok(false)
}

#[tauri::command]
pub fn get_clipboard_always_on_top(state: State<'_, AppState>) -> AppResult<bool> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    Ok(repo_get_always_on_top(&conn))
}

#[tauri::command]
pub fn set_clipboard_always_on_top(
    app: AppHandle,
    state: State<'_, AppState>,
    always_on_top: bool,
) -> AppResult<bool> {
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        repo_set_always_on_top(&conn, always_on_top)?;
    }
    if let Some(win) = app.get_webview_window("clipboard") {
        let _ = win.set_always_on_top(always_on_top);
    }
    Ok(always_on_top)
}

#[tauri::command]
pub fn list_clipboard_history(state: State<'_, AppState>) -> AppResult<Vec<ClipboardItem>> {
    let active_hash = state
        .active_clipboard_hash
        .read()
        .ok()
        .and_then(|g| g.clone());

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    clipboard_repo::list_clipboard_history(&conn, active_hash.as_deref())
}

/// Copies a historical `ClipboardItem` back into the active OS clipboard,
/// marks it as `CURRENTLY COPIED`, bumps its `copied_at` timestamp, and hides the `clipboard` drawer.
#[tauri::command]
pub fn paste_clipboard_item(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<ClipboardItem> {
    let mut item = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_clipboard_item(&conn, &id)?
            .ok_or_else(|| AppError::General("Clipboard item not found".to_string()))?
    };

    let decoded = image::open(&item.file_path)?;
    let rgba = decoded.to_rgba8();
    let tauri_img = tauri::image::Image::new(rgba.as_raw(), rgba.width(), rgba.height());
    app.clipboard()
        .write_image(&tauri_img)
        .map_err(|e| AppError::General(e.to_string()))?;

    if let Ok(mut active_guard) = state.active_clipboard_hash.write() {
        *active_guard = Some(item.blake3_hash.clone());
    }

    let now_iso = Utc::now().to_rfc3339();
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        bump_clipboard_item_timestamp(&conn, &id, &now_iso)?;
    }
    item.copied_at = now_iso;
    item.is_currently_copied = true;

    if let Some(clip_win) = app.get_webview_window("clipboard") {
        let _ = clip_win.hide();
    }

    let _ = app.emit("clipboard:updated", &item);
    Ok(item)
}

/// Loads any historical `ClipboardItem` directly into the `overlay` (`react-konva`) annotation
/// editor at its actual 1:1 dimensions, loading its clean `base_file_path` and restoring its
/// vector `annotations_json` so `Ctrl + Z` / `Ctrl + Y` works across sessions!
#[tauri::command]
pub fn annotate_clipboard_item(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<CaptureSessionMeta> {
    let item = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_clipboard_item(&conn, &id)?
            .ok_or_else(|| AppError::General("Clipboard item not found".to_string()))?
    };

    let editable_path = resolve_editable_clipboard_image_path(&item);
    let decoded = image::open(editable_path)?;
    let rgba = decoded.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let raw_bytes = rgba.clone().into_raw();

    let mut bmp_buf = Vec::with_capacity((width * height * 4 + 128) as usize);
    DynamicImage::ImageRgba8(rgba).write_to(&mut Cursor::new(&mut bmp_buf), ImageFormat::Bmp)?;

    let meta = CaptureSessionMeta {
        session_id: Uuid::new_v4().to_string(),
        monitor_id: 0,
        width,
        height,
        scale_factor: 1.0,
        captured_at: Utc::now().to_rfc3339(),
        source_clipboard_id: Some(item.id.clone()),
        initial_annotations_json: Some(item.annotations_json.clone()),
    };

    {
        let mut guard = state
            .latest_capture
            .write()
            .map_err(|_| AppError::General("Failed to lock capture state".to_string()))?;
        *guard = Some(FrozenFrame {
            meta: meta.clone(),
            bmp_bytes: Arc::new(bmp_buf),
            raw_rgba: Arc::new(raw_bytes),
        });
    }

    if let Some(clip_win) = app.get_webview_window("clipboard") {
        let _ = clip_win.hide();
    }

    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_fullscreen(true);
        let _ = overlay.set_always_on_top(true);
        let _ = overlay.show();
        let _ = overlay.set_focus();
        let _ = overlay.emit("capture:ready", &meta);
    }

    Ok(meta)
}

/// Permanently archives a `ClipboardItem` into the main Gallery working folder (`captures_dir`)
/// and indexes it in SQLite (`captures` + `captures_fts`).
#[tauri::command]
pub fn promote_clipboard_to_gallery(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> AppResult<CaptureRecord> {
    let item = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_clipboard_item(&conn, &id)?
            .ok_or_else(|| AppError::General("Clipboard item not found".to_string()))?
    };

    let decoded = image::open(&item.file_path)?;
    let rgba = decoded.to_rgba8();

    let captures_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let thumbs_dir = state
        .thumbs_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock thumbs_dir".to_string()))?
        .clone();

    let (file_path, thumb_path, size_bytes) = persist_raw_rgba_and_thumbnail(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        &captures_dir,
        &thumbs_dir,
        "webp",
    )?;

    let now_iso = Utc::now().to_rfc3339();
    let record = CaptureRecord {
        id: Uuid::new_v4().to_string(),
        media_type: "screenshot".to_string(),
        file_path,
        thumb_path,
        width: rgba.width(),
        height: rgba.height(),
        size_bytes,
        duration_ms: None,
        source_app: Some("Visual Clipboard (Alt + V)".to_string()),
        ocr_text: String::new(),
        is_pinned: item.is_pinned,
        group_name: String::new(),
        last_copied_at: Some(item.copied_at),
        tags: vec!["clipboard".to_string()],
        created_at: now_iso,
    };

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        insert_capture(&conn, &record)?;
    }

    let _ = app.emit("gallery:updated", &record);
    Ok(record)
}

#[tauri::command]
pub fn toggle_pin_clipboard_item(
    state: State<'_, AppState>,
    id: String,
    pinned: bool,
) -> AppResult<()> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    clipboard_repo::toggle_pin_clipboard_item(&conn, &id, pinned)
}

#[tauri::command]
pub fn delete_clipboard_item(state: State<'_, AppState>, id: String) -> AppResult<bool> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    clipboard_repo::delete_clipboard_item(&conn, &id)
}

#[tauri::command]
pub fn clear_unpinned_clipboard_history(state: State<'_, AppState>) -> AppResult<usize> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    clipboard_repo::clear_unpinned_clipboard_history(&conn)
}

#[tauri::command]
pub fn get_clipboard_paused(state: State<'_, AppState>) -> AppResult<bool> {
    Ok(state.clipboard_paused.load(Ordering::Relaxed))
}

#[tauri::command]
pub fn set_clipboard_paused(state: State<'_, AppState>, paused: bool) -> AppResult<bool> {
    state.clipboard_paused.store(paused, Ordering::Relaxed);
    Ok(paused)
}
