use std::path::Path;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardItem {
    pub id: String,
    pub blake3_hash: String,
    pub file_path: String,
    pub thumb_path: String,
    pub base_file_path: Option<String>,
    pub annotations_json: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub is_pinned: bool,
    pub is_currently_copied: bool,
    pub copied_at: String,
}

fn map_clipboard_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClipboardItem> {
    Ok(ClipboardItem {
        id: row.get(0)?,
        blake3_hash: row.get(1)?,
        file_path: row.get(2)?,
        thumb_path: row.get(3)?,
        base_file_path: row.get(4)?,
        annotations_json: row
            .get::<_, Option<String>>(5)?
            .unwrap_or_else(|| "[]".to_string()),
        width: row.get(6)?,
        height: row.get(7)?,
        size_bytes: row.get::<_, i64>(8)? as u64,
        is_pinned: row.get::<_, i64>(9)? != 0,
        is_currently_copied: false,
        copied_at: row.get(10)?,
    })
}

fn remove_cached_files(
    file_path: &str,
    thumb_path: &str,
    base_file_path: Option<&str>,
) {
    if !file_path.starts_with("data:") {
        let _ = std::fs::remove_file(file_path);
        let _ = std::fs::remove_file(thumb_path);
    }
    if let Some(base) = base_file_path {
        if !base.starts_with("data:") && base != file_path {
            let _ = std::fs::remove_file(base);
        }
    }
}

/// Checks if a clipboard image hash already exists in `clipboard_history` with valid files on disk.
/// If so, bumps its `copied_at` timestamp and returns `Some(existing_item)`.
pub fn try_bump_existing_hash(
    conn: &Connection,
    hash: &str,
    now_iso: &str,
) -> AppResult<Option<ClipboardItem>> {
    let existing: Option<ClipboardItem> = conn
        .query_row(
            "SELECT id, blake3_hash, file_path, thumb_path, base_file_path, annotations_json,
                    width, height, size_bytes, is_pinned, copied_at
             FROM clipboard_history
             WHERE blake3_hash = ?1",
            params![hash],
            map_clipboard_row,
        )
        .optional()?;

    if let Some(mut item) = existing {
        if item.file_path.starts_with("data:")
            || (Path::new(&item.file_path).exists() && Path::new(&item.thumb_path).exists())
        {
            conn.execute(
                "UPDATE clipboard_history SET copied_at = ?1 WHERE id = ?2",
                params![now_iso, item.id],
            )?;
            item.copied_at = now_iso.to_string();
            item.is_currently_copied = true;
            return Ok(Some(item));
        } else {
            let _ = conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![item.id]);
        }
    }

    Ok(None)
}

/// Inserts a new `ClipboardItem` into `clipboard_history` and enforces the FIFO ring buffer limit
/// (`max_unpinned_items`) by evicting the oldest unpinned items and deleting their cache files.
pub fn insert_and_evict_clipboard_item(
    conn: &Connection,
    item: &ClipboardItem,
    max_unpinned_items: u32,
) -> AppResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO clipboard_history (
            id, blake3_hash, file_path, thumb_path, base_file_path, annotations_json,
            width, height, size_bytes, is_pinned, copied_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            item.id,
            item.blake3_hash,
            item.file_path,
            item.thumb_path,
            item.base_file_path,
            item.annotations_json,
            item.width,
            item.height,
            item.size_bytes as i64,
            if item.is_pinned { 1 } else { 0 },
            item.copied_at,
        ],
    )?;

    enforce_fifo_limit(conn, max_unpinned_items)?;
    Ok(())
}

/// Updates an existing `ClipboardItem` after it has been edited in the Annotation Editor (`Alt + V -> Annotate`),
/// storing both the flattened `.webp`, the clean `base_file_path`, and the non-destructive `annotations_json`
/// so future edits retain full `Ctrl + Z` / `Ctrl + Y` history!
#[allow(clippy::too_many_arguments)]
pub fn update_clipboard_item_after_annotation(
    conn: &Connection,
    id: &str,
    new_hash: &str,
    file_path: &str,
    thumb_path: &str,
    base_file_path: Option<&str>,
    annotations_json: &str,
    width: u32,
    height: u32,
    size_bytes: u64,
    now_iso: &str,
) -> AppResult<()> {
    // Clean up old flattened file if path changed, but keep old base_file_path if reused
    if let Some(old) = get_clipboard_item(conn, id)? {
        if old.file_path != file_path && old.base_file_path.as_deref() != Some(&old.file_path) {
            let _ = std::fs::remove_file(&old.file_path);
            let _ = std::fs::remove_file(&old.thumb_path);
        }
        // Remove any other row that might collide on the new blake3_hash
        let _ = conn.execute(
            "DELETE FROM clipboard_history WHERE blake3_hash = ?1 AND id != ?2",
            params![new_hash, id],
        );
    }

    conn.execute(
        "UPDATE clipboard_history
         SET blake3_hash = ?1,
             file_path = ?2,
             thumb_path = ?3,
             base_file_path = COALESCE(?4, base_file_path, file_path),
             annotations_json = ?5,
             width = ?6,
             height = ?7,
             size_bytes = ?8,
             copied_at = ?9
         WHERE id = ?10",
        params![
            new_hash,
            file_path,
            thumb_path,
            base_file_path,
            annotations_json,
            width,
            height,
            size_bytes as i64,
            now_iso,
            id,
        ],
    )?;

    Ok(())
}

/// Evicts unpinned rows beyond `max_unpinned_items` (ordered newest-first by `copied_at`)
/// and removes their `.webp`, `_thumb.webp`, and `base_file_path` files from disk.
pub fn enforce_fifo_limit(conn: &Connection, max_unpinned_items: u32) -> AppResult<usize> {
    let safe_limit = max_unpinned_items.clamp(1, 200) as i64;

    let mut stmt = conn.prepare(
        "SELECT id, file_path, thumb_path, base_file_path
         FROM clipboard_history
         WHERE is_pinned = 0
         ORDER BY copied_at DESC
         LIMIT -1 OFFSET ?1",
    )?;

    let evicted: Vec<(String, String, String, Option<String>)> = stmt
        .query_map(params![safe_limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut removed = 0usize;
    for (id, file_path, thumb_path, base_file_path) in evicted {
        remove_cached_files(&file_path, &thumb_path, base_file_path.as_deref());
        removed += conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![id])?;
    }

    Ok(removed)
}

/// Reads the current `clipboard_max_items` setting from SQLite (defaults to 10).
pub fn get_clipboard_max_items(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'clipboard_max_items'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| serde_json::from_str::<u32>(&v).ok())
    .unwrap_or(10)
    .clamp(5, 100)
}

/// Reads the user's persistent `clipboard_always_on_top` preference (defaults to `true`).
pub fn get_clipboard_always_on_top(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'clipboard_always_on_top'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|v| serde_json::from_str::<bool>(&v).ok())
    .unwrap_or(true)
}

/// Persists the user's `clipboard_always_on_top` preference in SQLite `settings`.
pub fn set_clipboard_always_on_top(conn: &Connection, always_on_top: bool) -> AppResult<()> {
    let val = serde_json::to_string(&always_on_top)?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at)
         VALUES ('clipboard_always_on_top', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![val],
    )?;
    Ok(())
}

/// Lists all items in `clipboard_history` with **pinned items always on top** (`ORDER BY is_pinned DESC, copied_at DESC`),
/// marks the currently copied item (`is_currently_copied = true`), and prunes orphaned rows.
pub fn list_clipboard_history(
    conn: &Connection,
    active_hash: Option<&str>,
) -> AppResult<Vec<ClipboardItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, blake3_hash, file_path, thumb_path, base_file_path, annotations_json,
                width, height, size_bytes, is_pinned, copied_at
         FROM clipboard_history
         ORDER BY is_pinned DESC, copied_at DESC",
    )?;

    let rows: Vec<ClipboardItem> = stmt
        .query_map([], map_clipboard_row)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut valid = Vec::with_capacity(rows.len());
    for item in rows {
        if item.file_path.starts_with("data:") || Path::new(&item.file_path).exists() {
            valid.push(item);
        } else {
            let _ = conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![item.id]);
        }
    }

    // Mark which item is CURRENTLY COPIED in the active OS clipboard:
    // 1. Match `active_hash` if present
    // 2. Fallback to the item with the newest `copied_at` timestamp
    let mut matched = false;
    if let Some(hash) = active_hash {
        for item in &mut valid {
            if item.blake3_hash == hash {
                item.is_currently_copied = true;
                matched = true;
                break;
            }
        }
    }

    if !matched && !valid.is_empty() {
        if let Some(newest_idx) = valid
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.copied_at.cmp(&b.copied_at))
            .map(|(idx, _)| idx)
        {
            valid[newest_idx].is_currently_copied = true;
        }
    }

    Ok(valid)
}

pub fn get_clipboard_item(conn: &Connection, id: &str) -> AppResult<Option<ClipboardItem>> {
    conn.query_row(
        "SELECT id, blake3_hash, file_path, thumb_path, base_file_path, annotations_json,
                width, height, size_bytes, is_pinned, copied_at
         FROM clipboard_history
         WHERE id = ?1",
        params![id],
        map_clipboard_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn bump_clipboard_item_timestamp(
    conn: &Connection,
    id: &str,
    now_iso: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE clipboard_history SET copied_at = ?1 WHERE id = ?2",
        params![now_iso, id],
    )?;
    Ok(())
}

pub fn toggle_pin_clipboard_item(conn: &Connection, id: &str, pinned: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE clipboard_history SET is_pinned = ?1 WHERE id = ?2",
        params![if pinned { 1 } else { 0 }, id],
    )?;
    Ok(())
}

pub fn delete_clipboard_item(conn: &Connection, id: &str) -> AppResult<bool> {
    if let Some(item) = get_clipboard_item(conn, id)? {
        remove_cached_files(
            &item.file_path,
            &item.thumb_path,
            item.base_file_path.as_deref(),
        );
        let affected = conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![id])?;
        return Ok(affected > 0);
    }
    Ok(false)
}

pub fn clear_unpinned_clipboard_history(conn: &Connection) -> AppResult<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, thumb_path, base_file_path
         FROM clipboard_history
         WHERE is_pinned = 0",
    )?;
    let rows: Vec<(String, String, String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (_, file_path, thumb_path, base_file_path) in &rows {
        remove_cached_files(file_path, thumb_path, base_file_path.as_deref());
    }

    let removed = conn.execute("DELETE FROM clipboard_history WHERE is_pinned = 0", [])?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    #[test]
    fn test_clipboard_fifo_eviction_pinned_on_top_and_currently_copied() {
        let temp_db = std::env::temp_dir().join(format!(
            "rv_clip_test_{}.db",
            uuid::Uuid::new_v4()
        ));
        let conn = init_db(&temp_db).expect("init_db");

        // Insert 4 items with a max unpinned limit of 2; pin the oldest item (#1)
        for i in 1..=4 {
            let item = ClipboardItem {
                id: format!("clip-{}", i),
                blake3_hash: format!("hash-{}", i),
                file_path: format!("data:image/webp;base64,mock{}", i),
                thumb_path: format!("data:image/webp;base64,thumb{}", i),
                base_file_path: None,
                annotations_json: "[]".to_string(),
                width: 800,
                height: 600,
                size_bytes: 12_000,
                is_pinned: i == 1, // #1 is pinned!
                is_currently_copied: false,
                copied_at: format!("2026-11-15T08:00:0{}Z", i),
            };
            insert_and_evict_clipboard_item(&conn, &item, 2).expect("insert clip");
        }

        let list = list_clipboard_history(&conn, Some("hash-4")).expect("list clips");
        // Fix #4: Pinned item (#1) MUST stay at index 0 (on top!), followed by newest unpinned (#4, #3).
        // #2 was evicted by FIFO limit = 2!
        assert_eq!(list.len(), 3);
        let ids: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["clip-1", "clip-4", "clip-3"]);

        // Fix #5: Even though clip-1 is pinned at index 0, clip-4 has `is_currently_copied = true`!
        assert!(!list[0].is_currently_copied);
        assert!(list[1].is_currently_copied);

        // Clearing unpinned should leave only #1
        let cleared = clear_unpinned_clipboard_history(&conn).expect("clear unpinned");
        assert_eq!(cleared, 2);
        let remaining = list_clipboard_history(&conn, None).expect("remaining clips");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "clip-1");

        let _ = std::fs::remove_file(&temp_db);
    }
}
