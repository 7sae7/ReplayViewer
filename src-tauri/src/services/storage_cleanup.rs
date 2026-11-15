use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::captures_repo::{
    compute_storage_status_and_candidates, run_storage_cleanup_pass, StorageStatusSummary,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

fn read_storage_policy_settings(conn: &Connection) -> (u32, u32) {
    let max_gb: u32 = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'max_storage_gb'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| serde_json::from_str::<u32>(&v).ok())
        .unwrap_or(10);

    let cleanup_days: u32 = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'auto_cleanup_days'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| serde_json::from_str::<u32>(&v).ok())
        .unwrap_or(0);

    (max_gb, cleanup_days)
}

/// Computes a non-destructive Dry Run preview of the storage breakdown and which unpinned
/// captures would be moved into the 3-day "Queued Deletion" grace buffer.
pub fn get_storage_status_preview(
    app: &AppHandle,
    override_max_gb: Option<u32>,
    override_cleanup_days: Option<u32>,
) -> AppResult<StorageStatusSummary> {
    let state: tauri::State<'_, AppState> = app.state();
    let active_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

    let (saved_gb, saved_days) = read_storage_policy_settings(&conn);
    let max_gb = override_max_gb.unwrap_or(saved_gb);
    let cleanup_days = override_cleanup_days.unwrap_or(saved_days);
    let max_bytes = (max_gb as u64) * 1024 * 1024 * 1024;

    let (summary, _) = compute_storage_status_and_candidates(
        &conn,
        Some(&active_dir),
        max_bytes,
        cleanup_days,
    )?;
    Ok(summary)
}

/// Executes the storage policy pass:
/// 1. Permanently deletes unpinned items whose 3-day grace window has expired (`queued_delete_at <= now`).
/// 2. Queues newly pruned unpinned items for 3-day grace deletion (`queued_delete_at = now + 3 days`).
/// 3. Never touches pinned items (`is_pinned = 1`).
pub fn enforce_storage_policies_now(app: &AppHandle) -> AppResult<StorageStatusSummary> {
    let state: tauri::State<'_, AppState> = app.state();
    let active_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::General("Failed to lock database".to_string()))?;

    let (max_gb, cleanup_days) = read_storage_policy_settings(&conn);
    let max_bytes = (max_gb as u64) * 1024 * 1024 * 1024;

    let summary = run_storage_cleanup_pass(&conn, Some(&active_dir), max_bytes, cleanup_days)?;
    if summary.expired_purged_count > 0 || summary.queued_count > 0 {
        let _ = app.emit("gallery:updated", true);
    }
    Ok(summary)
}
