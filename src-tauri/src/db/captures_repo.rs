use std::collections::HashSet;
use std::path::{Path, PathBuf};
use chrono::{DateTime, Duration, Utc};
use image::ImageFormat;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::AppResult;

fn default_ocr_status() -> String {
    "pending".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRecord {
    pub id: String,
    pub media_type: String,
    pub file_path: String,
    pub thumb_path: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub duration_ms: Option<u64>,
    pub source_app: Option<String>,
    pub ocr_text: String,
    #[serde(default = "default_ocr_status")]
    pub ocr_status: String,
    pub is_pinned: bool,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub last_copied_at: Option<String>,
    #[serde(default)]
    pub queued_delete_at: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GalleryFilter {
    pub search_query: Option<String>,
    pub media_type: Option<String>,
    pub only_pinned: bool,
    #[serde(default)]
    pub only_recent_clipboard: bool,
    #[serde(default)]
    pub only_queued_deletion: bool,
    #[serde(default)]
    pub group_name: Option<String>,
    pub tag: Option<String>,
    #[serde(default)]
    pub date_range: Option<String>,
    #[serde(default)]
    pub sort_by: Option<String>,
    #[serde(default)]
    pub sort_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedCaptures {
    pub items: Vec<CaptureRecord>,
    pub next_cursor: Option<String>,
    pub total_count: u64,
    pub total_bytes: u64,
    pub queued_deletion_count: u64,
    pub queued_deletion_bytes: u64,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedCandidateSample {
    pub id: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub created_at: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatusSummary {
    pub active_count: u64,
    pub active_bytes: u64,
    pub pinned_count: u64,
    pub pinned_bytes: u64,
    pub unpinned_count: u64,
    pub unpinned_bytes: u64,
    pub queued_count: u64,
    pub queued_bytes: u64,
    pub next_expiry_at: Option<String>,
    pub candidate_count: u64,
    pub candidate_bytes: u64,
    pub candidate_samples: Vec<QueuedCandidateSample>,
    pub expired_purged_count: u64,
    pub pending_ocr_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnindexedScanResult {
    pub folder_path: String,
    pub count: usize,
    pub total_bytes: u64,
    pub sample_names: Vec<String>,
}

fn normalize_dir_prefix(dir: &Path) -> String {
    let s = dir
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
        .replace('\'', "''");
    format!("{}/%", s)
}

pub fn insert_capture(conn: &Connection, record: &CaptureRecord) -> AppResult<()> {
    let status = if record.ocr_status.is_empty() {
        if record.ocr_text.is_empty() {
            "pending"
        } else {
            "done"
        }
    } else {
        record.ocr_status.as_str()
    };

    conn.execute(
        r#"
        INSERT INTO captures (
            id, media_type, file_path, thumb_path, width, height,
            size_bytes, duration_ms, source_app, ocr_text, ocr_status,
            is_pinned, group_name, last_copied_at, queued_delete_at, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
        "#,
        params![
            record.id,
            record.media_type,
            record.file_path,
            record.thumb_path,
            record.width,
            record.height,
            record.size_bytes as i64,
            record.duration_ms.map(|d| d as i64),
            record.source_app,
            record.ocr_text,
            status,
            if record.is_pinned { 1 } else { 0 },
            record.group_name,
            record.last_copied_at,
            record.queued_delete_at,
            record.created_at,
        ],
    )?;

    for tag in &record.tags {
        let trimmed = tag.trim().to_lowercase();
        if !trimmed.is_empty() {
            conn.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![trimmed],
            )?;
            if let Ok(tag_id) = conn.query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![trimmed],
                |r| r.get::<_, i64>(0),
            ) {
                let _ = conn.execute(
                    "INSERT OR IGNORE INTO capture_tags (capture_id, tag_id) VALUES (?1, ?2)",
                    params![record.id, tag_id],
                );
            }
        }
    }

    let joined_tags = format!("{} {}", record.tags.join(" "), record.group_name);
    conn.execute(
        "INSERT INTO captures_fts (capture_id, source_app, ocr_text, tag_names) VALUES (?1, ?2, ?3, ?4)",
        params![
            record.id,
            record.source_app.as_deref().unwrap_or(""),
            record.ocr_text,
            joined_tags.trim(),
        ],
    )?;

    Ok(())
}

pub fn list_captures(
    conn: &Connection,
    cursor: Option<String>,
    limit: u32,
    filter: &GalleryFilter,
    active_dir: Option<&Path>,
) -> AppResult<PaginatedCaptures> {
    let safe_limit = limit.clamp(1, 5000);

    let dir_clause = if let Some(dir) = active_dir {
        let prefix = normalize_dir_prefix(dir);
        format!(" AND REPLACE(LOWER(file_path), '\\', '/') LIKE '{}' ", prefix)
    } else {
        String::new()
    };

    let count_sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM captures WHERE queued_delete_at IS NULL {}",
        dir_clause
    );
    let (total_count, total_bytes): (i64, i64) =
        conn.query_row(&count_sql, [], |row| Ok((row.get(0)?, row.get(1)?)))?;

    let queued_sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM captures WHERE queued_delete_at IS NOT NULL AND is_pinned = 0 {}",
        dir_clause
    );
    let (queued_count, queued_bytes): (i64, i64) =
        conn.query_row(&queued_sql, [], |row| Ok((row.get(0)?, row.get(1)?)))?;

    let groups_sql = format!(
        "SELECT DISTINCT group_name FROM captures WHERE group_name != '' AND queued_delete_at IS NULL {} ORDER BY group_name ASC",
        dir_clause
    );
    let mut groups_stmt = conn.prepare(&groups_sql)?;
    let groups_iter = groups_stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut groups = Vec::new();
    for g in groups_iter {
        groups.push(g?);
    }

    let mut sql = String::from(
        r#"
        SELECT c.id, c.media_type, c.file_path, c.thumb_path, c.width, c.height,
               c.size_bytes, c.duration_ms, c.source_app, c.ocr_text,
               COALESCE(c.ocr_status, 'pending'), c.is_pinned,
               c.group_name, c.last_copied_at, c.queued_delete_at, c.created_at
        FROM captures c
        "#,
    );

    let search = filter
        .search_query
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    if search.is_some() {
        sql.push_str(" JOIN captures_fts fts ON fts.capture_id = c.id ");
    }

    sql.push_str(" WHERE 1 = 1 ");
    if let Some(dir) = active_dir {
        let prefix = normalize_dir_prefix(dir);
        sql.push_str(&format!(
            " AND REPLACE(LOWER(c.file_path), '\\', '/') LIKE '{}' ",
            prefix
        ));
    }

    if filter.only_queued_deletion {
        sql.push_str(" AND c.queued_delete_at IS NOT NULL AND c.is_pinned = 0 ");
    } else {
        sql.push_str(" AND c.queued_delete_at IS NULL ");
    }

    let mut fts_match_arg = String::new();
    if let Some(q) = search {
        let sanitized: String = q
            .chars()
            .filter(|ch| ch.is_alphanumeric() || ch.is_whitespace())
            .collect();
        let terms: Vec<String> = sanitized
            .split_whitespace()
            .map(|t| format!("\"{}\"*", t))
            .collect();
        if !terms.is_empty() {
            fts_match_arg = terms.join(" AND ");
            sql.push_str(" AND captures_fts MATCH ?1 ");
        }
    }

    if filter.only_pinned {
        sql.push_str(" AND c.is_pinned = 1 ");
    }

    if filter.only_recent_clipboard {
        sql.push_str(" AND c.last_copied_at IS NOT NULL ");
    }

    if let Some(ref media_type) = filter.media_type {
        if media_type == "screenshot" || media_type == "video" {
            sql.push_str(&format!(" AND c.media_type = '{}' ", media_type));
        }
    }

    if let Some(ref g_name) = filter.group_name {
        let trimmed = g_name.trim();
        if !trimmed.is_empty() {
            let safe_group: String = trimmed.chars().filter(|ch| *ch != '\'').collect();
            sql.push_str(&format!(" AND c.group_name = '{}' ", safe_group));
        }
    }

    if let Some(ref dr) = filter.date_range {
        let cutoff = match dr.as_str() {
            "today" => Some(Utc::now() - Duration::hours(24)),
            "7d" => Some(Utc::now() - Duration::days(7)),
            "30d" => Some(Utc::now() - Duration::days(30)),
            _ => None,
        };
        if let Some(c_time) = cutoff {
            sql.push_str(&format!(" AND c.created_at >= '{}' ", c_time.to_rfc3339()));
        }
    }

    let sort_dir_sql = if filter.only_queued_deletion && filter.sort_dir.is_none() {
        "ASC"
    } else {
        match filter.sort_dir.as_deref() {
            Some("asc") | Some("ASC") => "ASC",
            _ => "DESC",
        }
    };

    let sort_col_sql = if filter.only_queued_deletion
        && filter.sort_by.as_deref().unwrap_or("created_at") == "created_at"
    {
        "c.queued_delete_at"
    } else if filter.only_recent_clipboard
        && filter.sort_by.as_deref().unwrap_or("created_at") == "created_at"
    {
        "c.last_copied_at"
    } else {
        match filter.sort_by.as_deref() {
            Some("size_bytes") => "c.size_bytes",
            Some("last_copied_at") => "c.last_copied_at",
            _ => "c.created_at",
        }
    };

    if let Some(ref c_val) = cursor {
        let safe_cursor: String = c_val.chars().filter(|ch| *ch != '\'').collect();
        let cmp = if sort_dir_sql == "ASC" { ">" } else { "<" };
        sql.push_str(&format!(" AND c.created_at {} '{}' ", cmp, safe_cursor));
    }

    sql.push_str(&format!(
        " ORDER BY {} {} LIMIT {} ",
        sort_col_sql, sort_dir_sql, safe_limit
    ));

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = if !fts_match_arg.is_empty() {
        stmt.query(params![fts_match_arg])?
    } else {
        stmt.query([])?
    };

    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let tags = get_tags_for_capture(conn, &id)?;
        items.push(CaptureRecord {
            id,
            media_type: row.get(1)?,
            file_path: row.get(2)?,
            thumb_path: row.get(3)?,
            width: row.get::<_, i64>(4)? as u32,
            height: row.get::<_, i64>(5)? as u32,
            size_bytes: row.get::<_, i64>(6)? as u64,
            duration_ms: row.get::<_, Option<i64>>(7)?.map(|d| d as u64),
            source_app: row.get(8)?,
            ocr_text: row.get(9)?,
            ocr_status: row.get(10)?,
            is_pinned: row.get::<_, i64>(11)? != 0,
            group_name: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
            last_copied_at: row.get(13)?,
            queued_delete_at: row.get(14)?,
            tags,
            created_at: row.get(15)?,
        });
    }

    let next_cursor = if items.len() as u32 == safe_limit {
        items.last().map(|last| last.created_at.clone())
    } else {
        None
    };

    Ok(PaginatedCaptures {
        items,
        next_cursor,
        total_count: total_count as u64,
        total_bytes: total_bytes as u64,
        queued_deletion_count: queued_count as u64,
        queued_deletion_bytes: queued_bytes as u64,
        groups,
    })
}

fn get_tags_for_capture(conn: &Connection, capture_id: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT t.name
        FROM tags t
        JOIN capture_tags ct ON ct.tag_id = t.id
        WHERE ct.capture_id = ?1
        ORDER BY t.name ASC
        "#,
    )?;
    let rows = stmt.query_map(params![capture_id], |r| r.get::<_, String>(0))?;
    let mut tags = Vec::new();
    for r in rows {
        tags.push(r?);
    }
    Ok(tags)
}

/// Pinning a capture immediately rescues it from any queued deletion (`queued_delete_at = NULL`).
/// Pinned captures (`is_pinned = 1`) are 100% exempt from storage quota and retention pruning.
pub fn toggle_pin_capture(conn: &Connection, id: &str, pinned: bool) -> AppResult<()> {
    if pinned {
        conn.execute(
            "UPDATE captures SET is_pinned = 1, queued_delete_at = NULL WHERE id = ?1",
            params![id],
        )?;
    } else {
        conn.execute(
            "UPDATE captures SET is_pinned = 0 WHERE id = ?1",
            params![id],
        )?;
    }
    Ok(())
}

pub fn mark_capture_copied(conn: &Connection, file_path: &str) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE captures SET last_copied_at = ?1 WHERE file_path = ?2 OR id = ?2",
        params![now, file_path],
    )?;
    Ok(())
}

pub fn batch_add_tags(
    conn: &mut Connection,
    capture_ids: &[String],
    tag_names: &[String],
) -> AppResult<usize> {
    let clean_tags: Vec<String> = tag_names
        .iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    if clean_tags.is_empty() || capture_ids.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction()?;
    for tag in &clean_tags {
        tx.execute(
            "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
            params![tag],
        )?;
    }

    for cap_id in capture_ids {
        for tag in &clean_tags {
            let tag_id: i64 = tx.query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                params![tag],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO capture_tags (capture_id, tag_id) VALUES (?1, ?2)",
                params![cap_id, tag_id],
            )?;
        }

        let tags = get_tags_for_capture(&tx, cap_id)?;
        let group_name: String = tx
            .query_row(
                "SELECT COALESCE(group_name, '') FROM captures WHERE id = ?1",
                params![cap_id],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let joined = format!("{} {}", tags.join(" "), group_name);
        let _ = tx.execute(
            "UPDATE captures_fts SET tag_names = ?1 WHERE capture_id = ?2",
            params![joined.trim(), cap_id],
        );
    }

    tx.commit()?;
    Ok(capture_ids.len())
}

pub fn batch_set_group(
    conn: &mut Connection,
    capture_ids: &[String],
    group_name: &str,
) -> AppResult<usize> {
    let clean_group = group_name.trim();
    let tx = conn.transaction()?;

    for cap_id in capture_ids {
        tx.execute(
            "UPDATE captures SET group_name = ?1 WHERE id = ?2",
            params![clean_group, cap_id],
        )?;
        let tags = get_tags_for_capture(&tx, cap_id)?;
        let joined = format!("{} {}", tags.join(" "), clean_group);
        let _ = tx.execute(
            "UPDATE captures_fts SET tag_names = ?1 WHERE capture_id = ?2",
            params![joined.trim(), cap_id],
        );
    }

    tx.commit()?;
    Ok(capture_ids.len())
}

/// Moves the selected captures' files into `target_dir` on disk, cleans up their
/// thumbnails in the working directory, and removes them from the gallery SQLite DB.
pub fn move_captures_to_folder(
    conn: &mut Connection,
    capture_ids: &[String],
    target_dir: &Path,
) -> AppResult<usize> {
    std::fs::create_dir_all(target_dir)?;
    let mut moved = 0;

    for id in capture_ids {
        let paths: Option<(String, String)> = conn
            .query_row(
                "SELECT file_path, thumb_path FROM captures WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((file_path, thumb_path)) = paths {
            let src = Path::new(&file_path);
            if src.exists() {
                let file_name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| format!("{}.webp", id));
                let mut dest = target_dir.join(&file_name);
                if dest.exists() && dest != src {
                    let stem = src
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| id.clone());
                    let ext = src
                        .extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_else(|| "webp".to_string());
                    dest = target_dir.join(format!("{}_{}.{}", stem, &id[..8.min(id.len())], ext));
                }

                if dest != src {
                    if std::fs::rename(src, &dest).is_err() {
                        std::fs::copy(src, &dest)?;
                        let _ = std::fs::remove_file(src);
                    }
                }
            }
            let _ = std::fs::remove_file(&thumb_path);
        }

        moved += conn.execute("DELETE FROM captures WHERE id = ?1", params![id])?;
        let _ = conn.execute("DELETE FROM captures_fts WHERE capture_id = ?1", params![id]);
    }

    Ok(moved)
}

/// Migrates all captures belonging to `old_dir` into `new_dir` (and `new_thumbs_dir`),
/// moving their files on disk and updating `file_path` and `thumb_path` in SQLite
/// so all tags, pins, groups, and timestamps remain intact!
pub fn migrate_captures_to_folder(
    conn: &mut Connection,
    old_dir: &Path,
    new_dir: &Path,
    new_thumbs_dir: &Path,
) -> AppResult<usize> {
    std::fs::create_dir_all(new_dir)?;
    std::fs::create_dir_all(new_thumbs_dir)?;

    let prefix = normalize_dir_prefix(old_dir);
    let sql = format!(
        "SELECT id, file_path, thumb_path FROM captures WHERE REPLACE(LOWER(file_path), '\\', '/') LIKE '{}'",
        prefix
    );

    let rows_data: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare(&sql)?;
        let mapped = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut v = Vec::new();
        for item in mapped {
            v.push(item?);
        }
        v
    };

    let tx = conn.transaction()?;
    let mut migrated = 0;

    for (id, file_path, thumb_path) in rows_data {
        let src_file = Path::new(&file_path);
        let file_name = src_file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.webp", id));
        let dest_file = new_dir.join(&file_name);

        if src_file.exists() && src_file != dest_file {
            if std::fs::rename(src_file, &dest_file).is_err() {
                let _ = std::fs::copy(src_file, &dest_file);
                let _ = std::fs::remove_file(src_file);
            }
        }

        let src_thumb = Path::new(&thumb_path);
        let thumb_name = src_thumb
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}_thumb.webp", id));
        let dest_thumb = new_thumbs_dir.join(&thumb_name);

        if src_thumb.exists() && src_thumb != dest_thumb {
            if std::fs::rename(src_thumb, &dest_thumb).is_err() {
                let _ = std::fs::copy(src_thumb, &dest_thumb);
                let _ = std::fs::remove_file(src_thumb);
            }
        }

        tx.execute(
            "UPDATE captures SET file_path = ?1, thumb_path = ?2 WHERE id = ?3",
            params![
                dest_file.to_string_lossy().to_string(),
                dest_thumb.to_string_lossy().to_string(),
                id
            ],
        )?;
        migrated += 1;
    }

    tx.commit()?;
    Ok(migrated)
}

fn is_supported_image_ext(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_lowercase().as_str(),
        "webp" | "png" | "jpg" | "jpeg" | "bmp"
    )
}

/// Scans `captures_dir` for top-level image files that are not yet indexed in SQLite.
pub fn scan_unindexed_media(
    conn: &Connection,
    captures_dir: &Path,
) -> AppResult<UnindexedScanResult> {
    if !captures_dir.exists() {
        return Ok(UnindexedScanResult {
            folder_path: captures_dir.to_string_lossy().to_string(),
            count: 0,
            total_bytes: 0,
            sample_names: Vec::new(),
        });
    }

    let mut indexed_paths = HashSet::new();
    {
        let mut existing_stmt = conn.prepare("SELECT file_path FROM captures")?;
        let existing_iter = existing_stmt.query_map([], |r| r.get::<_, String>(0))?;
        for p in existing_iter {
            if let Ok(path_str) = p {
                indexed_paths.insert(path_str.replace('\\', "/").to_lowercase());
            }
        }
    }

    let mut count = 0;
    let mut total_bytes = 0u64;
    let mut sample_names = Vec::new();

    for entry in std::fs::read_dir(captures_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !is_supported_image_ext(&path) {
            continue;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name.starts_with('.') || file_name.starts_with("_bench_sample_") {
            continue;
        }

        let norm = path.to_string_lossy().replace('\\', "/").to_lowercase();
        if !indexed_paths.contains(&norm) {
            count += 1;
            if let Ok(meta) = entry.metadata() {
                total_bytes += meta.len();
            }
            if sample_names.len() < 4 {
                sample_names.push(file_name);
            }
        }
    }

    Ok(UnindexedScanResult {
        folder_path: captures_dir.to_string_lossy().to_string(),
        count,
        total_bytes,
        sample_names,
    })
}

/// Imports all unindexed top-level images inside `captures_dir` into SQLite,
/// preserving their original filesystem modification dates and generating 320px WebP thumbnails.
pub fn import_external_media(
    conn: &mut Connection,
    captures_dir: &Path,
    thumbs_dir: &Path,
) -> AppResult<usize> {
    std::fs::create_dir_all(captures_dir)?;
    std::fs::create_dir_all(thumbs_dir)?;

    let mut indexed_paths = HashSet::new();
    {
        let mut existing_stmt = conn.prepare("SELECT file_path FROM captures")?;
        let existing_iter = existing_stmt.query_map([], |r| r.get::<_, String>(0))?;
        for p in existing_iter {
            if let Ok(path_str) = p {
                indexed_paths.insert(path_str.replace('\\', "/").to_lowercase());
            }
        }
    }

    let mut to_import: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(captures_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !is_supported_image_ext(&path) {
            continue;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name.starts_with('.') || file_name.starts_with("_bench_sample_") {
            continue;
        }

        let norm = path.to_string_lossy().replace('\\', "/").to_lowercase();
        if !indexed_paths.contains(&norm) {
            to_import.push(path);
        }
    }

    if to_import.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction()?;
    let mut imported_count = 0;

    for path in to_import {
        let Ok(decoded) = image::open(&path) else {
            continue;
        };
        let width = decoded.width();
        let height = decoded.height();
        let meta = std::fs::metadata(&path)?;
        let size_bytes = meta.len();

        // Preserve original file modification/creation timestamp for accurate timeline sorting!
        let created_at = meta
            .modified()
            .or_else(|_| meta.created())
            .map(|st| DateTime::<Utc>::from(st).to_rfc3339())
            .unwrap_or_else(|_| Utc::now().to_rfc3339());

        let id = Uuid::new_v4().to_string();
        let thumb_path = thumbs_dir.join(format!("{}_thumb.webp", id));
        let thumb = decoded.thumbnail(320, 320);
        if thumb.save_with_format(&thumb_path, ImageFormat::WebP).is_err() {
            let _ = thumb.save(&path);
        }

        let file_stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported Image".to_string());

        let record = CaptureRecord {
            id,
            media_type: "screenshot".to_string(),
            file_path: path.to_string_lossy().to_string(),
            thumb_path: thumb_path.to_string_lossy().to_string(),
            width,
            height,
            size_bytes,
            duration_ms: None,
            source_app: Some(file_stem),
            ocr_text: String::new(),
            ocr_status: "pending".to_string(),
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: None,
            queued_delete_at: None,
            tags: vec!["imported".to_string()],
            created_at,
        };

        insert_capture(&tx, &record)?;
        imported_count += 1;
    }

    tx.commit()?;
    Ok(imported_count)
}

pub fn delete_captures(conn: &Connection, ids: &[String]) -> AppResult<usize> {
    let mut deleted = 0;
    for id in ids {
        let paths: Option<(String, String)> = conn
            .query_row(
                "SELECT file_path, thumb_path FROM captures WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((file_path, thumb_path)) = paths {
            let _ = std::fs::remove_file(&file_path);
            let _ = std::fs::remove_file(&thumb_path);
        }

        deleted += conn.execute("DELETE FROM captures WHERE id = ?1", params![id])?;
        let _ = conn.execute("DELETE FROM captures_fts WHERE capture_id = ?1", params![id]);
    }
    Ok(deleted)
}

/// Restores captures from the 3-Day "Queued Deletion" grace period (`queued_delete_at = NULL`).
/// If `also_pin` is true, also marks them `is_pinned = 1` so future storage quota passes never re-queue them.
pub fn restore_queued_captures(
    conn: &Connection,
    ids: &[String],
    also_pin: bool,
) -> AppResult<usize> {
    let mut restored = 0;
    for id in ids {
        if also_pin {
            restored += conn.execute(
                "UPDATE captures SET queued_delete_at = NULL, is_pinned = 1 WHERE id = ?1",
                params![id],
            )?;
        } else {
            restored += conn.execute(
                "UPDATE captures SET queued_delete_at = NULL WHERE id = ?1",
                params![id],
            )?;
        }
    }
    Ok(restored)
}

/// Updates extracted OCR text (`captures.ocr_text`, `captures.ocr_status`) and syncs `captures_fts`
/// so `< 2 ms` full-text search immediately matches newly recognized words.
pub fn update_capture_ocr(
    conn: &Connection,
    id: &str,
    ocr_text: &str,
    status: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE captures SET ocr_text = ?1, ocr_status = ?2 WHERE id = ?3",
        params![ocr_text, status, id],
    )?;
    conn.execute(
        "UPDATE captures_fts SET ocr_text = ?1 WHERE capture_id = ?2",
        params![ocr_text, id],
    )?;
    Ok(())
}

/// Returns up to `limit` active screenshots whose OCR has not yet been scanned (`ocr_status = 'pending'`),
/// ordered newest-first (and pinned-first) so recent captures become searchable first.
pub fn list_pending_ocr_captures(
    conn: &Connection,
    active_dir: Option<&Path>,
    limit: usize,
) -> AppResult<Vec<(String, String)>> {
    let dir_clause = if let Some(dir) = active_dir {
        let prefix = normalize_dir_prefix(dir);
        format!(" AND REPLACE(LOWER(file_path), '\\', '/') LIKE '{}' ", prefix)
    } else {
        String::new()
    };

    let sql = format!(
        r#"
        SELECT id, file_path
        FROM captures
        WHERE COALESCE(ocr_status, 'pending') = 'pending'
          AND media_type = 'screenshot'
          AND queued_delete_at IS NULL
          {}
        ORDER BY is_pinned DESC, created_at DESC
        LIMIT {}
        "#,
        dir_clause,
        limit.clamp(1, 1000)
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Computes the live storage breakdown and evaluates which unpinned captures would be moved
/// into the 3-day "Queued Deletion" buffer under `max_storage_bytes` (`0` = unlimited) and
/// `auto_cleanup_days` (`0` = no max retention age).
/// Pinned captures (`is_pinned = 1`) are 100% exempt and never selected.
pub fn compute_storage_status_and_candidates(
    conn: &Connection,
    active_dir: Option<&Path>,
    max_storage_bytes: u64,
    auto_cleanup_days: u32,
) -> AppResult<(StorageStatusSummary, Vec<String>)> {
    let dir_clause = if let Some(dir) = active_dir {
        let prefix = normalize_dir_prefix(dir);
        format!(" AND REPLACE(LOWER(file_path), '\\', '/') LIKE '{}' ", prefix)
    } else {
        String::new()
    };

    let (pinned_count, pinned_bytes): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM captures WHERE is_pinned = 1 AND queued_delete_at IS NULL {}",
            dir_clause
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let (unpinned_count, unpinned_bytes): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM captures WHERE is_pinned = 0 AND queued_delete_at IS NULL {}",
            dir_clause
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    let (queued_count, queued_bytes, next_expiry_at): (i64, i64, Option<String>) = conn.query_row(
        &format!(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0), MIN(queued_delete_at) FROM captures WHERE is_pinned = 0 AND queued_delete_at IS NOT NULL {}",
            dir_clause
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;

    let pending_ocr_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM captures WHERE COALESCE(ocr_status, 'pending') = 'pending' AND media_type = 'screenshot' AND queued_delete_at IS NULL {}",
            dir_clause
        ),
        [],
        |r| r.get(0),
    )?;

    let active_count = (pinned_count + unpinned_count) as u64;
    let active_bytes = (pinned_bytes + unpinned_bytes) as u64;

    // Fetch all unpinned, non-queued captures ordered oldest -> newest (for fair FIFO quota/age pruning)
    let mut stmt = conn.prepare(&format!(
        r#"
        SELECT id, file_path, size_bytes, created_at
        FROM captures
        WHERE is_pinned = 0 AND queued_delete_at IS NULL {}
        ORDER BY created_at ASC
        "#,
        dir_clause
    ))?;

    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)? as u64,
            r.get::<_, String>(3)?,
        ))
    })?;

    let mut unpinned_items = Vec::new();
    for r in rows {
        unpinned_items.push(r?);
    }

    let age_cutoff_iso = if auto_cleanup_days > 0 {
        Some((Utc::now() - Duration::days(auto_cleanup_days as i64)).to_rfc3339())
    } else {
        None
    };

    let mut candidate_ids = Vec::new();
    let mut candidate_bytes = 0u64;
    let mut candidate_samples = Vec::new();
    let mut selected_set = HashSet::new();

    // Pass 1: Optional Retention Age rule (`auto_cleanup_days > 0`)
    if let Some(ref cutoff) = age_cutoff_iso {
        for (id, file_path, size, created_at) in &unpinned_items {
            if created_at < cutoff {
                selected_set.insert(id.clone());
                candidate_ids.push(id.clone());
                candidate_bytes += *size;
                if candidate_samples.len() < 8 {
                    let file_name = Path::new(file_path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| file_path.clone());
                    candidate_samples.push(QueuedCandidateSample {
                        id: id.clone(),
                        file_name,
                        size_bytes: *size,
                        created_at: created_at.clone(),
                        reason: format!("Older than {} days", auto_cleanup_days),
                    });
                }
            }
        }
    }

    // Pass 2: Max Storage Quota rule (`max_storage_bytes > 0`)
    if max_storage_bytes > 0 && active_bytes.saturating_sub(candidate_bytes) > max_storage_bytes {
        let mut remaining_bytes = active_bytes.saturating_sub(candidate_bytes);
        for (id, file_path, size, created_at) in &unpinned_items {
            if remaining_bytes <= max_storage_bytes {
                break;
            }
            if selected_set.contains(id) {
                continue;
            }
            selected_set.insert(id.clone());
            candidate_ids.push(id.clone());
            candidate_bytes += *size;
            remaining_bytes = remaining_bytes.saturating_sub(*size);
            if candidate_samples.len() < 8 {
                let file_name = Path::new(file_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| file_path.clone());
                candidate_samples.push(QueuedCandidateSample {
                    id: id.clone(),
                    file_name,
                    size_bytes: *size,
                    created_at: created_at.clone(),
                    reason: "Exceeds folder GB quota".to_string(),
                });
            }
        }
    }

    let summary = StorageStatusSummary {
        active_count,
        active_bytes,
        pinned_count: pinned_count as u64,
        pinned_bytes: pinned_bytes as u64,
        unpinned_count: unpinned_count as u64,
        unpinned_bytes: unpinned_bytes as u64,
        queued_count: queued_count as u64,
        queued_bytes: queued_bytes as u64,
        next_expiry_at,
        candidate_count: candidate_ids.len() as u64,
        candidate_bytes,
        candidate_samples,
        expired_purged_count: 0,
        pending_ocr_count: pending_ocr_count as u64,
    };

    Ok((summary, candidate_ids))
}

/// Runs the automated storage policy pass:
/// 1. Permanently deletes any unpinned captures whose 3-day "Queued Deletion" grace period has expired (`queued_delete_at <= now`).
/// 2. Evaluates quota (`max_storage_bytes`) and optional retention age (`auto_cleanup_days`) and moves excess
///    unpinned captures into the 3-day "Queued Deletion" grace state (`queued_delete_at = now + 3 days`).
/// 3. Never touches pinned captures (`is_pinned = 1`).
pub fn run_storage_cleanup_pass(
    conn: &Connection,
    active_dir: Option<&Path>,
    max_storage_bytes: u64,
    auto_cleanup_days: u32,
) -> AppResult<StorageStatusSummary> {
    let now = Utc::now();
    let now_iso = now.to_rfc3339();

    // 1. Rescue any accidentally queued pinned items first, then permanently delete expired unpinned queued items
    let _ = conn.execute(
        "UPDATE captures SET queued_delete_at = NULL WHERE is_pinned = 1 AND queued_delete_at IS NOT NULL",
        [],
    );

    let expired_ids: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM captures WHERE is_pinned = 0 AND queued_delete_at IS NOT NULL AND queued_delete_at <= ?1",
        )?;
        let rows = stmt.query_map(params![now_iso], |r| r.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(r?);
        }
        ids
    };

    let expired_purged_count = if !expired_ids.is_empty() {
        delete_captures(conn, &expired_ids)? as u64
    } else {
        0
    };

    // 2. Compute any new unpinned candidates exceeding quota or retention age
    let (_pre_summary, candidate_ids) = compute_storage_status_and_candidates(
        conn,
        active_dir,
        max_storage_bytes,
        auto_cleanup_days,
    )?;

    if !candidate_ids.is_empty() {
        let expiry_iso = (now + Duration::days(3)).to_rfc3339();
        for id in &candidate_ids {
            conn.execute(
                "UPDATE captures SET queued_delete_at = ?1 WHERE id = ?2 AND is_pinned = 0",
                params![expiry_iso, id],
            )?;
        }
    }

    let (mut post_summary, _) = compute_storage_status_and_candidates(
        conn,
        active_dir,
        max_storage_bytes,
        auto_cleanup_days,
    )?;
    post_summary.expired_purged_count = expired_purged_count;
    Ok(post_summary)
}

/// Phase 1.0 Guarantee #4: Reconcile SQLite records against the filesystem
/// so files manually deleted or moved in Windows Explorer / Finder are pruned cleanly.
pub fn reconcile_orphaned_captures(conn: &Connection) -> AppResult<usize> {
    let mut stmt = conn.prepare("SELECT id, file_path, thumb_path FROM captures")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut orphaned_ids = Vec::new();
    for row in rows {
        let (id, file_path, thumb_path) = row?;
        if !Path::new(&file_path).exists() {
            let _ = std::fs::remove_file(&thumb_path);
            orphaned_ids.push(id);
        }
    }

    let count = orphaned_ids.len();
    for id in orphaned_ids {
        conn.execute("DELETE FROM captures WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM captures_fts WHERE capture_id = ?1", params![id])?;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    #[test]
    fn test_fts5_search_sorting_groups_and_orphan_reconciliation() {
        let mut conn = init_db(Path::new(":memory:")).expect("in-memory sqlite init");

        let rec1 = CaptureRecord {
            id: "cap-1".to_string(),
            media_type: "screenshot".to_string(),
            file_path: "C:/FolderA/non_existent_1.webp".to_string(),
            thumb_path: "C:/FolderA/.thumbs/non_existent_1_thumb.webp".to_string(),
            width: 1920,
            height: 1080,
            size_bytes: 250_000,
            duration_ms: None,
            source_app: Some("Valorant".to_string()),
            ocr_text: "MATCH MVP SCOREBOARD".to_string(),
            ocr_status: "done".to_string(),
            is_pinned: true,
            group_name: String::new(),
            last_copied_at: None,
            queued_delete_at: None,
            tags: vec!["clutch".to_string(), "gaming".to_string()],
            created_at: "2026-11-12T10:00:00Z".to_string(),
        };

        let rec2 = CaptureRecord {
            id: "cap-2".to_string(),
            media_type: "screenshot".to_string(),
            file_path: "C:/FolderB/non_existent_2.webp".to_string(),
            thumb_path: "C:/FolderB/.thumbs/non_existent_2_thumb.webp".to_string(),
            width: 1920,
            height: 1080,
            size_bytes: 180_000,
            duration_ms: None,
            source_app: Some("Chrome".to_string()),
            ocr_text: "HTTP 500 Internal Server Error".to_string(),
            ocr_status: "done".to_string(),
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: Some("2026-11-12T10:06:00Z".to_string()),
            queued_delete_at: None,
            tags: vec!["bug".to_string()],
            created_at: "2026-11-12T10:05:00Z".to_string(),
        };

        insert_capture(&conn, &rec1).unwrap();
        insert_capture(&conn, &rec2).unwrap();

        // 1. Test FTS5 query for "500"
        let search_res = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                search_query: Some("500".to_string()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(search_res.items.len(), 1);
        assert_eq!(search_res.items[0].id, "cap-2");

        // 2. Test Active Directory Scoping (FolderA vs FolderB)
        let folder_a_res = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter::default(),
            Some(Path::new("C:\\FolderA")),
        )
        .unwrap();
        assert_eq!(folder_a_res.items.len(), 1);
        assert_eq!(folder_a_res.items[0].id, "cap-1");

        // 3. Test Sort Direction (ASC vs DESC)
        let asc_res = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                sort_dir: Some("asc".to_string()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(asc_res.items[0].id, "cap-1");
        assert_eq!(asc_res.items[1].id, "cap-2");

        // 4. Test Batch Group Assignment
        batch_set_group(&mut conn, &["cap-1".to_string()], "Ranked Highlights").unwrap();
        let group_res = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                group_name: Some("Ranked Highlights".to_string()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(group_res.items.len(), 1);
        assert_eq!(group_res.groups, vec!["Ranked Highlights".to_string()]);

        // 5. Test Recently Copied Filter
        let recent_clip = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                only_recent_clipboard: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(recent_clip.items.len(), 1);
        assert_eq!(recent_clip.items[0].id, "cap-2");

        // 6. Test Orphaned DB Row Reconciliation (both files do not exist on disk)
        let pruned = reconcile_orphaned_captures(&conn).unwrap();
        assert_eq!(pruned, 2);

        let after_prune =
            list_captures(&conn, None, 50, &GalleryFilter::default(), None).unwrap();
        assert_eq!(after_prune.total_count, 0);
    }

    #[test]
    fn test_storage_quota_3_day_grace_period_pinned_immunity_and_ocr_update() {
        let conn = init_db(Path::new(":memory:")).expect("in-memory sqlite init");

        // Oldest item, but PINNED (must NEVER be queued for deletion!)
        let pinned_old = CaptureRecord {
            id: "pinned-old".to_string(),
            media_type: "screenshot".to_string(),
            file_path: "C:/Cap/pinned.webp".to_string(),
            thumb_path: "C:/Cap/.thumbs/pinned.webp".to_string(),
            width: 1920,
            height: 1080,
            size_bytes: 600_000,
            duration_ms: None,
            source_app: Some("Discord".to_string()),
            ocr_text: String::new(),
            ocr_status: "pending".to_string(),
            is_pinned: true,
            group_name: String::new(),
            last_copied_at: None,
            queued_delete_at: None,
            tags: vec![],
            created_at: "2026-10-01T10:00:00Z".to_string(),
        };

        // Unpinned older item (should be queued for 3-day deletion when quota is 1 MB)
        let unpinned_old = CaptureRecord {
            id: "unpinned-old".to_string(),
            media_type: "screenshot".to_string(),
            file_path: "C:/Cap/unpinned_old.webp".to_string(),
            thumb_path: "C:/Cap/.thumbs/unpinned_old.webp".to_string(),
            width: 1920,
            height: 1080,
            size_bytes: 500_000,
            duration_ms: None,
            source_app: Some("Chrome".to_string()),
            ocr_text: String::new(),
            ocr_status: "pending".to_string(),
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: None,
            queued_delete_at: None,
            tags: vec![],
            created_at: "2026-11-01T10:00:00Z".to_string(),
        };

        // Unpinned newer item (fits within 1 MB quota once unpinned-old is queued)
        let unpinned_new = CaptureRecord {
            id: "unpinned-new".to_string(),
            media_type: "screenshot".to_string(),
            file_path: "C:/Cap/unpinned_new.webp".to_string(),
            thumb_path: "C:/Cap/.thumbs/unpinned_new.webp".to_string(),
            width: 1920,
            height: 1080,
            size_bytes: 300_000,
            duration_ms: None,
            source_app: Some("VS Code".to_string()),
            ocr_text: String::new(),
            ocr_status: "pending".to_string(),
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: None,
            queued_delete_at: None,
            tags: vec![],
            created_at: "2026-11-14T10:00:00Z".to_string(),
        };

        insert_capture(&conn, &pinned_old).unwrap();
        insert_capture(&conn, &unpinned_old).unwrap();
        insert_capture(&conn, &unpinned_new).unwrap();

        // Total active bytes = 1.4 MB (600KB pinned + 800KB unpinned).
        // Enforce 1.0 MB (1_000_000 bytes) quota with 0 retention days:
        // pinned-old MUST be untouched; unpinned-old (500KB) is queued for 3-day deletion, bringing active bytes to 900KB!
        let summary = run_storage_cleanup_pass(&conn, None, 1_000_000, 0).unwrap();
        assert_eq!(summary.pinned_count, 1);
        assert_eq!(summary.queued_count, 1);
        assert_eq!(summary.active_count, 2);

        // Verify Queued Deletion view has `unpinned-old` with a 3-day expiry timestamp
        let queued_page = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                only_queued_deletion: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(queued_page.items.len(), 1);
        assert_eq!(queued_page.items[0].id, "unpinned-old");
        assert!(queued_page.items[0].queued_delete_at.is_some());

        // Restore & Pin `unpinned-old` -> must return to active gallery and be pinned
        restore_queued_captures(&conn, &["unpinned-old".to_string()], true).unwrap();
        let after_restore =
            list_captures(&conn, None, 50, &GalleryFilter::default(), None).unwrap();
        assert_eq!(after_restore.total_count, 3);
        assert_eq!(after_restore.queued_deletion_count, 0);

        // Test OCR background update & FTS5 sync
        update_capture_ocr(&conn, "unpinned-new", "Fatal Exception 0x80070005", "done").unwrap();
        let ocr_search = list_captures(
            &conn,
            None,
            50,
            &GalleryFilter {
                search_query: Some("0x80070005".to_string()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(ocr_search.items.len(), 1);
        assert_eq!(ocr_search.items[0].id, "unpinned-new");
        assert_eq!(ocr_search.items[0].ocr_status, "done");
    }
}
