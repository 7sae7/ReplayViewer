use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use uuid::Uuid;

use crate::db::captures_repo::{insert_capture, CaptureRecord};
use crate::error::{AppError, AppResult};
use crate::services::capture::{
    freeze_active_screen, persist_capture_and_thumbnail, persist_raw_rgba_and_thumbnail,
    ScreenCaptureProvider, XcapCaptureProvider,
};
use crate::state::{AppState, CaptureSessionMeta};

/// Triggered via `Alt + S` or UI button: freezes active screen in RAM as an uncompressed BMP (< 3ms)
/// BEFORE showing the pre-warmed fullscreen `overlay` window.
#[tauri::command]
pub fn trigger_region_capture(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<CaptureSessionMeta> {
    let frozen = freeze_active_screen(&XcapCaptureProvider)?;
    let meta = frozen.meta.clone();

    {
        let mut guard = state
            .latest_capture
            .write()
            .map_err(|_| AppError::General("Failed to lock capture state".to_string()))?;
        *guard = Some(frozen);
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

/// Phase 1.0 Guarantee #2: `Alt + Shift + S` Instant Fullscreen Capture.
/// Grabs raw RGBA monitor pixels, copies to clipboard immediately, and writes WebP + thumbnail
/// without double-encoding through PNG, eliminating capture lag.
#[tauri::command]
pub fn trigger_fullscreen_capture(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<CaptureRecord> {
    let raw = XcapCaptureProvider.capture_active_monitor()?;

    // Copy raw RGBA image to OS clipboard immediately
    let rgba_img = tauri::image::Image::new(&raw.rgba_bytes, raw.width, raw.height);
    let _ = app.clipboard().write_image(&rgba_img);

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
        &raw.rgba_bytes,
        raw.width,
        raw.height,
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
        width: raw.width,
        height: raw.height,
        size_bytes,
        duration_ms: None,
        source_app: Some("Fullscreen Capture".to_string()),
        ocr_text: String::new(),
        is_pinned: false,
        group_name: String::new(),
        last_copied_at: Some(now_iso.clone()),
        tags: vec!["fullscreen".to_string()],
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

/// Receives the flattened, cropped & annotated PNG/WebP bytes from the `overlay` window,
/// hides the overlay immediately for instant UX, copies to clipboard, and persists to SQLite.
#[tauri::command]
pub fn save_annotated_capture(
    app: AppHandle,
    state: State<'_, AppState>,
    image_bytes: Vec<u8>,
    copy_to_clipboard: bool,
    image_format: Option<String>,
    source_app: Option<String>,
) -> AppResult<CaptureRecord> {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_fullscreen(false);
        let _ = overlay.hide();
    }

    if copy_to_clipboard {
        if let Ok(decoded) = image::load_from_memory(&image_bytes) {
            let rgba = decoded.to_rgba8();
            let tauri_img = tauri::image::Image::new(rgba.as_raw(), rgba.width(), rgba.height());
            let _ = app.clipboard().write_image(&tauri_img);
        }
    }

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

    let ext = image_format.as_deref().unwrap_or("webp");
    let (file_path, thumb_path, width, height, size_bytes) = persist_capture_and_thumbnail(
        &image_bytes,
        &captures_dir,
        &thumbs_dir,
        ext,
    )?;

    let now_iso = Utc::now().to_rfc3339();
    let record = CaptureRecord {
        id: Uuid::new_v4().to_string(),
        media_type: "screenshot".to_string(),
        file_path,
        thumb_path,
        width,
        height,
        size_bytes,
        duration_ms: None,
        source_app: source_app.or_else(|| Some("Region Capture".to_string())),
        ocr_text: String::new(),
        is_pinned: false,
        group_name: String::new(),
        last_copied_at: if copy_to_clipboard {
            Some(now_iso.clone())
        } else {
            None
        },
        tags: vec!["annotated".to_string()],
        created_at: now_iso,
    };

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        insert_capture(&conn, &record)?;
    }

    if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.show();
        let _ = main_win.set_focus();
    }

    let _ = app.emit("gallery:updated", &record);
    Ok(record)
}
