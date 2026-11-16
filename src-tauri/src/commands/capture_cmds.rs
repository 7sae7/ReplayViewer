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
use crate::services::clipboard::{
    ingest_rgba_into_clipboard_history, save_or_update_annotated_clipboard_item,
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

    // Copy raw RGBA image to OS clipboard immediately & record in Alt+V Visual Clipboard ring
    let rgba_img = tauri::image::Image::new(&raw.rgba_bytes, raw.width, raw.height);
    let _ = app.clipboard().write_image(&rgba_img);
    let _ = ingest_rgba_into_clipboard_history(&app, &raw.rgba_bytes, raw.width, raw.height);

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
        ocr_status: "pending".to_string(),
        is_pinned: false,
        group_name: String::new(),
        last_copied_at: Some(now_iso.clone()),
        queued_delete_at: None,
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
    crate::services::ocr::spawn_background_ocr_and_storage_check(
        app,
        record.id.clone(),
        record.file_path.clone(),
    );
    Ok(record)
}

/// Receives the flattened, cropped & annotated PNG/WebP bytes from the `overlay` window,
/// hides the overlay immediately for instant UX, and supports 3 distinct export modes:
/// 1. `Copy Only` (`copy_to_clipboard = true, save_to_disk = Some(false)` via `Ctrl+C` / `Cmd+C`):
///    Copies to the OS clipboard and `Alt + V` ring without writing to the PC's Gallery `captures_dir`.
/// 2. `Save Only` (`copy_to_clipboard = false, save_to_disk = Some(true)`):
///    Saves to `captures_dir` and SQLite Gallery without overwriting the OS clipboard.
/// 3. `Copy & Save` (`copy_to_clipboard = true, save_to_disk = Some(true)` via `Enter`):
///    Copies to the OS clipboard AND saves to `captures_dir` + SQLite Gallery.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn save_annotated_capture(
    app: AppHandle,
    state: State<'_, AppState>,
    image_bytes: Vec<u8>,
    copy_to_clipboard: bool,
    save_to_disk: Option<bool>,
    image_format: Option<String>,
    source_app: Option<String>,
    base_image_bytes: Option<Vec<u8>>,
    annotations_json: Option<String>,
    source_clipboard_id: Option<String>,
) -> AppResult<CaptureRecord> {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_fullscreen(false);
        let _ = overlay.hide();
    }

    let should_save_to_disk = save_to_disk.unwrap_or(true);
    let mut decoded_dims = (0u32, 0u32);

    if let Ok(decoded) = image::load_from_memory(&image_bytes) {
        let rgba = decoded.to_rgba8();
        decoded_dims = (rgba.width(), rgba.height());
        if copy_to_clipboard || !should_save_to_disk || source_clipboard_id.is_some() {
            let tauri_img = tauri::image::Image::new(rgba.as_raw(), rgba.width(), rgba.height());
            let _ = app.clipboard().write_image(&tauri_img);
            let _ = save_or_update_annotated_clipboard_item(
                &app,
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                base_image_bytes.as_deref(),
                annotations_json.as_deref(),
                source_clipboard_id.as_deref(),
            );
        }
    }

    let now_iso = Utc::now().to_rfc3339();
    let default_source = if source_clipboard_id.is_some() {
        "Clipboard Annotation (Alt + V)"
    } else {
        "Region Capture"
    };

    // Mode 1: Copy Only (`Ctrl+C` / `Cmd+C` or `Copy` button) — do NOT save a file into `captures_dir`
    if !should_save_to_disk {
        if source_clipboard_id.is_some() {
            if let Some(clip_win) = app.get_webview_window("clipboard") {
                let _ = clip_win.show();
                let _ = clip_win.set_focus();
                let _ = clip_win.emit("clipboard:opened", true);
            }
        } else {
            let _ = app.emit(
                "shortcut:toast",
                "Copied to clipboard (Not saved to disk — press Alt+V for clipboard history)",
            );
        }

        return Ok(CaptureRecord {
            id: format!("copied-only-{}", Uuid::new_v4()),
            media_type: "screenshot".to_string(),
            file_path: String::new(),
            thumb_path: String::new(),
            width: decoded_dims.0,
            height: decoded_dims.1,
            size_bytes: image_bytes.len() as u64,
            duration_ms: None,
            source_app: source_app.or_else(|| Some(default_source.to_string())),
            ocr_text: String::new(),
            ocr_status: "done".to_string(),
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: Some(now_iso.clone()),
            queued_delete_at: None,
            tags: Vec::new(),
            created_at: now_iso,
        });
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

    let record = CaptureRecord {
        id: Uuid::new_v4().to_string(),
        media_type: "screenshot".to_string(),
        file_path,
        thumb_path,
        width,
        height,
        size_bytes,
        duration_ms: None,
        source_app: source_app.or_else(|| Some(default_source.to_string())),
        ocr_text: String::new(),
        ocr_status: "pending".to_string(),
        is_pinned: false,
        group_name: String::new(),
        last_copied_at: if copy_to_clipboard || source_clipboard_id.is_some() {
            Some(now_iso.clone())
        } else {
            None
        },
        queued_delete_at: None,
        tags: if source_clipboard_id.is_some() {
            vec!["annotated".to_string(), "clipboard".to_string()]
        } else {
            vec!["annotated".to_string()]
        },
        created_at: now_iso,
    };

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        insert_capture(&conn, &record)?;
    }

    if source_clipboard_id.is_some() {
        // If user launched Annotate from Alt+V, reopen Alt+V so they immediately see their saved edit!
        if let Some(clip_win) = app.get_webview_window("clipboard") {
            let _ = clip_win.show();
            let _ = clip_win.set_focus();
            let _ = clip_win.emit("clipboard:opened", true);
        }
    } else if let Some(main_win) = app.get_webview_window("main") {
        let _ = main_win.show();
        let _ = main_win.set_focus();
    }

    let _ = app.emit("gallery:updated", &record);
    crate::services::ocr::spawn_background_ocr_and_storage_check(
        app,
        record.id.clone(),
        record.file_path.clone(),
    );
    Ok(record)
}
