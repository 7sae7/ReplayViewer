use std::path::Path;
use std::process::Command;
use chrono::{Duration, Utc};
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use uuid::Uuid;

use crate::db::captures_repo::{
    self, insert_capture, CaptureRecord, GalleryFilter, PaginatedCaptures, UnindexedScanResult,
};
use crate::error::{AppError, AppResult};
use crate::services::capture::ensure_benchmark_sample_image;
use crate::services::clipboard::ingest_rgba_into_clipboard_history;
use crate::state::AppState;

#[tauri::command]
pub fn list_captures(
    state: State<'_, AppState>,
    cursor: Option<String>,
    limit: u32,
    filter: GalleryFilter,
) -> AppResult<PaginatedCaptures> {
    let active_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::list_captures(&conn, cursor, limit, &filter, Some(&active_dir))
}

#[tauri::command]
pub fn scan_unindexed_media(state: State<'_, AppState>) -> AppResult<UnindexedScanResult> {
    let active_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::scan_unindexed_media(&conn, &active_dir)
}

#[tauri::command]
pub fn import_external_media(state: State<'_, AppState>) -> AppResult<usize> {
    let active_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let thumbs_dir = state
        .thumbs_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock thumbs_dir".to_string()))?
        .clone();
    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::import_external_media(&mut conn, &active_dir, &thumbs_dir)
}

#[tauri::command]
pub fn migrate_captures_to_folder(
    state: State<'_, AppState>,
    old_dir: String,
    new_dir: String,
) -> AppResult<usize> {
    let old_path = Path::new(old_dir.trim());
    let new_path = Path::new(new_dir.trim());
    let new_thumbs = new_path.join(".thumbs");

    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::migrate_captures_to_folder(&mut conn, old_path, new_path, &new_thumbs)
}

#[tauri::command]
pub fn toggle_pin_capture(
    state: State<'_, AppState>,
    id: String,
    pinned: bool,
) -> AppResult<()> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::toggle_pin_capture(&conn, &id, pinned)
}

#[tauri::command]
pub fn delete_captures(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> AppResult<usize> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::delete_captures(&conn, &ids)
}

#[tauri::command]
pub fn batch_add_tags(
    state: State<'_, AppState>,
    capture_ids: Vec<String>,
    tag_names: Vec<String>,
) -> AppResult<usize> {
    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::batch_add_tags(&mut conn, &capture_ids, &tag_names)
}

#[tauri::command]
pub fn batch_set_group(
    state: State<'_, AppState>,
    capture_ids: Vec<String>,
    group_name: String,
) -> AppResult<usize> {
    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::batch_set_group(&mut conn, &capture_ids, &group_name)
}

#[tauri::command]
pub fn move_captures_to_folder(
    state: State<'_, AppState>,
    capture_ids: Vec<String>,
    target_dir: String,
) -> AppResult<usize> {
    let trimmed = target_dir.trim();
    if trimmed.is_empty() {
        return Err(AppError::General("Target directory cannot be empty".to_string()));
    }
    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::move_captures_to_folder(&mut conn, &capture_ids, Path::new(trimmed))
}

#[tauri::command]
pub fn reconcile_orphaned_captures(state: State<'_, AppState>) -> AppResult<usize> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
    captures_repo::reconcile_orphaned_captures(&conn)
}

#[tauri::command]
pub fn copy_capture_to_clipboard(
    app: AppHandle,
    state: State<'_, AppState>,
    file_path: String,
) -> AppResult<()> {
    let decoded = image::open(&file_path)?;
    let rgba = decoded.to_rgba8();
    let tauri_img = tauri::image::Image::new(rgba.as_raw(), rgba.width(), rgba.height());
    app.clipboard()
        .write_image(&tauri_img)
        .map_err(|e| AppError::General(e.to_string()))?;

    if let Ok(conn) = state.db.lock() {
        let _ = captures_repo::mark_capture_copied(&conn, &file_path);
    }
    let _ = ingest_rgba_into_clipboard_history(&app, rgba.as_raw(), rgba.width(), rgba.height());
    Ok(())
}

/// Opens a native OS Folder Picker dialog so users can choose a new Storage Directory
/// or pick a destination folder when moving multi-selected captures.
#[tauri::command]
pub fn pick_folder_dialog(default_path: Option<String>) -> AppResult<Option<String>> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let init_dir = default_path.unwrap_or_default().replace('\'', "''");
        let ps_script = format!(
            r#"
            Add-Type -AssemblyName System.Windows.Forms
            $dlg = New-Object System.Windows.Forms.FolderBrowserDialog
            $dlg.Description = 'Select Folder for ReplayViewer'
            $dlg.ShowNewFolderButton = $true
            if ('{init_dir}' -ne '' -and (Test-Path -LiteralPath '{init_dir}')) {{
                $dlg.SelectedPath = '{init_dir}'
            }}
            if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{
                Write-Output $dlg.SelectedPath
            }}
            "#
        );

        let output = Command::new("powershell")
            .args(["-NoProfile", "-Sta", "-Command", &ps_script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;

        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if chosen.is_empty() {
            return Ok(None);
        }
        return Ok(Some(chosen));
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"Select Folder for ReplayViewer\")",
            ])
            .output()?;
        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(if chosen.is_empty() { None } else { Some(chosen) });
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("zenity")
            .args(["--file-selection", "--directory", "--title=Select Folder"])
            .output()?;
        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(if chosen.is_empty() { None } else { Some(chosen) });
    }
}

/// Opens the given file or folder inside the native OS File Manager
/// (Windows File Explorer with the file highlighted, macOS Finder, or Linux xdg-open).
#[tauri::command]
pub fn open_in_file_explorer(path: String) -> AppResult<()> {
    let target = Path::new(&path);

    #[cfg(target_os = "windows")]
    {
        if target.is_file() {
            Command::new("explorer.exe")
                .arg(format!("/select,{}", target.display()))
                .spawn()?;
        } else {
            let _ = std::fs::create_dir_all(target);
            Command::new("explorer.exe").arg(target).spawn()?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if target.is_file() {
            Command::new("open").arg("-R").arg(target).spawn()?;
        } else {
            let _ = std::fs::create_dir_all(target);
            Command::new("open").arg(target).spawn()?;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let dir = if target.is_file() {
            target.parent().unwrap_or(target)
        } else {
            let _ = std::fs::create_dir_all(target);
            target
        };
        Command::new("xdg-open").arg(dir).spawn()?;
    }

    Ok(())
}

/// Inserts `count` synthetic benchmark captures into the real SQLite + FTS5 database
/// inside a single transaction (< 20ms) so users can stress-test 60 FPS virtualized scrolling.
#[tauri::command]
pub fn seed_benchmark_captures(
    state: State<'_, AppState>,
    count: u32,
) -> AppResult<usize> {
    let safe_count = count.clamp(1, 2000) as usize;

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

    let mut samples = Vec::with_capacity(5);
    for v in 0..5 {
        samples.push(ensure_benchmark_sample_image(
            &captures_dir,
            &thumbs_dir,
            v,
        )?);
    }

    let apps = [
        ("Valorant", "MATCH MVP 14-12 SCOREBOARD", "valorant", "Esports"),
        ("Chrome DevTools", "HTTP 500 Internal Server Error", "bug", "Bug Reports"),
        ("VS Code", "Rust SQLite FTS5 Virtualized Grid", "code", "Architecture"),
        ("Cyberpunk 2077", "Raytracing Benchmark 144 FPS", "gaming", "Benchmarks"),
        ("Blender", "Viewport Render Shader Node", "design", ""),
    ];

    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

    let tx = conn.transaction()?;
    let now = Utc::now();

    for i in 0..safe_count {
        let (ref file_path, ref thumb_path, size_bytes) = samples[i % samples.len()];
        let (app, ocr, tag, group) = apps[i % apps.len()];
        let created_at = (now - Duration::seconds((i as i64) * 30)).to_rfc3339();

        let rec = CaptureRecord {
            id: Uuid::new_v4().to_string(),
            media_type: "screenshot".to_string(),
            file_path: file_path.clone(),
            thumb_path: thumb_path.clone(),
            width: 1920,
            height: 1080,
            size_bytes: size_bytes + ((i as u64 % 50) * 1024),
            duration_ms: None,
            source_app: Some(format!("{} #{}", app, i + 1)),
            ocr_text: format!("{} item {}", ocr, i + 1),
            is_pinned: i % 25 == 0,
            group_name: group.to_string(),
            last_copied_at: if i % 20 == 0 {
                Some(created_at.clone())
            } else {
                None
            },
            tags: vec![tag.to_string(), "benchmark".to_string()],
            created_at,
        };
        insert_capture(&tx, &rec)?;
    }

    tx.commit()?;
    Ok(safe_count)
}

#[tauri::command]
pub fn get_latest_capture_meta(
    state: State<'_, AppState>,
) -> AppResult<Option<crate::state::CaptureSessionMeta>> {
    let guard = state
        .latest_capture
        .read()
        .map_err(|_| AppError::General("Failed to lock capture state".to_string()))?;
    Ok(guard.as_ref().map(|f| f.meta.clone()))
}

#[tauri::command]
pub fn add_tag_to_capture(
    state: State<'_, AppState>,
    capture_id: String,
    tag_name: String,
) -> AppResult<CaptureRecord> {
    let trimmed = tag_name.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err(AppError::General("Tag name cannot be empty".to_string()));
    }

    let mut conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

    captures_repo::batch_add_tags(&mut conn, &[capture_id.clone()], &[trimmed])?;

    let page = captures_repo::list_captures(
        &conn,
        None,
        5000,
        &GalleryFilter::default(),
        None,
    )?;
    let updated = page
        .items
        .into_iter()
        .find(|i| i.id == capture_id)
        .ok_or_else(|| AppError::General("Capture not found".to_string()))?;

    Ok(updated)
}
