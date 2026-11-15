use std::collections::HashSet;
use std::path::{Path, PathBuf};
use chrono::{DateTime, Duration, Utc};
use image::ImageFormat;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::AppResult;

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
    pub is_pinned: bool,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub last_copied_at: Option<String>,
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
    pub groups: Vec<String>,
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
    conn.execute(
        r#"
        INSERT INTO captures (
            id, media_type, file_path, thumb_path, width, height,
            size_bytes, duration_ms, source_app, ocr_text, is_pinned,
            group_name, last_copied_at, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
            if record.is_pinned { 1 } else { 0 },
            record.group_name,
            record.last_copied_at,
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
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM captures WHERE 1 = 1 {}",
        dir_clause
    );
    let (total_count, total_bytes): (i64, i64) =
        conn.query_row(&count_sql, [], |row| Ok((row.get(0)?, row.get(1)?)))?;

    let groups_sql = format!(
        "SELECT DISTINCT group_name FROM captures WHERE group_name != '' {} ORDER BY group_name ASC",
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
               c.size_bytes, c.duration_ms, c.source_app, c.ocr_text, c.is_pinned,
               c.group_name, c.last_copied_at, c.created_at
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

    let sort_dir_sql = match filter.sort_dir.as_deref() {
        Some("asc") | Some("ASC") => "ASC",
        _ => "DESC",
    };

    let sort_col_sql = if filter.only_recent_clipboard
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
            is_pinned: row.get::<_, i64>(10)? != 0,
            group_name: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
            last_copied_at: row.get(12)?,
            tags,
            created_at: row.get(13)?,
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

pub fn toggle_pin_capture(conn: &Connection, id: &str, pinned: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE captures SET is_pinned = ?1 WHERE id = ?2",
        params![if pinned { 1 } else { 0 }, id],
    )?;
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
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: None,
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
            is_pinned: true,
            group_name: String::new(),
            last_copied_at: None,
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
            is_pinned: false,
            group_name: String::new(),
            last_copied_at: Some("2026-11-12T10:06:00Z".to_string()),
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
}
