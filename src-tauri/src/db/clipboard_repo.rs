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
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub is_pinned: bool,
    pub copied_at: String,
}

/// Checks if a clipboard image hash already exists in `clipboard_history` with valid files on disk.
/// If so, bumps its `copied_at` timestamp to the top of the drawer and returns `Some(existing_item)`.
pub fn try_bump_existing_hash(
    conn: &Connection,
    hash: &str,
    now_iso: &str,
) -> AppResult<Option<ClipboardItem>> {
    let existing: Option<ClipboardItem> = conn
        .query_row(
            "SELECT id, blake3_hash, file_path, thumb_path, width, height, size_bytes, is_pinned, copied_at
             FROM clipboard_history
             WHERE blake3_hash = ?1",
            params![hash],
            |row| {
                Ok(ClipboardItem {
                    id: row.get(0)?,
                    blake3_hash: row.get(1)?,
                    file_path: row.get(2)?,
                    thumb_path: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                    size_bytes: row.get::<_, i64>(6)? as u64,
                    is_pinned: row.get::<_, i64>(7)? != 0,
                    copied_at: row.get(8)?,
                })
            },
        )
        .optional()?;

    if let Some(mut item) = existing {
        if Path::new(&item.file_path).exists() && Path::new(&item.thumb_path).exists() {
            conn.execute(
                "UPDATE clipboard_history SET copied_at = ?1 WHERE id = ?2",
                params![now_iso, item.id],
            )?;
            item.copied_at = now_iso.to_string();
            return Ok(Some(item));
        } else {
            // Stale row whose disk cache was manually deleted; remove it so a fresh copy can be written
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
            id, blake3_hash, file_path, thumb_path, width, height, size_bytes, is_pinned, copied_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            item.id,
            item.blake3_hash,
            item.file_path,
            item.thumb_path,
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

/// Evicts unpinned rows beyond `max_unpinned_items` (ordered newest-first by `copied_at`)
/// and removes their `.webp` and `_thumb.webp` files from disk.
pub fn enforce_fifo_limit(conn: &Connection, max_unpinned_items: u32) -> AppResult<usize> {
    let safe_limit = max_unpinned_items.clamp(1, 200) as i64;

    let mut stmt = conn.prepare(
        "SELECT id, file_path, thumb_path
         FROM clipboard_history
         WHERE is_pinned = 0
         ORDER BY copied_at DESC
         LIMIT -1 OFFSET ?1",
    )?;

    let evicted: Vec<(String, String, String)> = stmt
        .query_map(params![safe_limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut removed = 0usize;
    for (id, file_path, thumb_path) in evicted {
        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_file(&thumb_path);
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

/// Lists all items in `clipboard_history` ordered newest-first (`copied_at DESC`),
/// automatically pruning any orphaned rows whose cache files were deleted on disk.
pub fn list_clipboard_history(conn: &Connection) -> AppResult<Vec<ClipboardItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, blake3_hash, file_path, thumb_path, width, height, size_bytes, is_pinned, copied_at
         FROM clipboard_history
         ORDER BY copied_at DESC",
    )?;

    let rows: Vec<ClipboardItem> = stmt
        .query_map([], |row| {
            Ok(ClipboardItem {
                id: row.get(0)?,
                blake3_hash: row.get(1)?,
                file_path: row.get(2)?,
                thumb_path: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                size_bytes: row.get::<_, i64>(6)? as u64,
                is_pinned: row.get::<_, i64>(7)? != 0,
                copied_at: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut valid = Vec::with_capacity(rows.len());
    for item in rows {
        if item.file_path.starts_with("data:") || Path::new(&item.file_path).exists() {
            valid.push(item);
        } else {
            let _ = conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![item.id]);
        }
    }

    Ok(valid)
}

pub fn get_clipboard_item(conn: &Connection, id: &str) -> AppResult<Option<ClipboardItem>> {
    conn.query_row(
        "SELECT id, blake3_hash, file_path, thumb_path, width, height, size_bytes, is_pinned, copied_at
         FROM clipboard_history
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(ClipboardItem {
                id: row.get(0)?,
                blake3_hash: row.get(1)?,
                file_path: row.get(2)?,
                thumb_path: row.get(3)?,
                width: row.get(4)?,
                height: row.get(5)?,
                size_bytes: row.get::<_, i64>(6)? as u64,
                is_pinned: row.get::<_, i64>(7)? != 0,
                copied_at: row.get(8)?,
            })
        },
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
        if !item.file_path.starts_with("data:") {
            let _ = std::fs::remove_file(&item.file_path);
            let _ = std::fs::remove_file(&item.thumb_path);
        }
        let affected = conn.execute("DELETE FROM clipboard_history WHERE id = ?1", params![id])?;
        return Ok(affected > 0);
    }
    Ok(false)
}

pub fn clear_unpinned_clipboard_history(conn: &Connection) -> AppResult<usize> {
    let mut stmt = conn.prepare(
        "SELECT id, file_path, thumb_path FROM clipboard_history WHERE is_pinned = 0",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    for (_, file_path, thumb_path) in &rows {
        if !file_path.starts_with("data:") {
            let _ = std::fs::remove_file(file_path);
            let _ = std::fs::remove_file(thumb_path);
        }
    }

    let removed = conn.execute("DELETE FROM clipboard_history WHERE is_pinned = 0", [])?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    #[test]
    fn test_clipboard_fifo_eviction_and_pin_protection() {
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
                width: 800,
                height: 600,
                size_bytes: 12_000,
                is_pinned: i == 1, // #1 is pinned!
                copied_at: format!("2026-11-15T08:00:0{}Z", i),
            };
            insert_and_evict_clipboard_item(&conn, &item, 2).expect("insert clip");
        }

        let list = list_clipboard_history(&conn).expect("list clips");
        // Should contain: #4 (unpinned newest), #3 (unpinned 2nd newest), and #1 (pinned!)
        // #2 should have been evicted by FIFO limit = 2!
        assert_eq!(list.len(), 3);
        let ids: Vec<&str> = list.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["clip-4", "clip-3", "clip-1"]);

        // Clearing unpinned should leave only #1
        let cleared = clear_unpinned_clipboard_history(&conn).expect("clear unpinned");
        assert_eq!(cleared, 2);
        let remaining = list_clipboard_history(&conn).expect("remaining clips");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "clip-1");

        let _ = std::fs::remove_file(&temp_db);
    }
}
