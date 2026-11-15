pub mod captures_repo;
pub mod clipboard_repo;

use std::path::Path;
use rusqlite::Connection;
use crate::error::AppResult;

const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS captures (
    id              TEXT PRIMARY KEY,
    media_type      TEXT NOT NULL CHECK (media_type IN ('screenshot', 'video')),
    file_path       TEXT NOT NULL,
    thumb_path      TEXT NOT NULL,
    width           INTEGER NOT NULL,
    height          INTEGER NOT NULL,
    size_bytes      INTEGER NOT NULL,
    duration_ms     INTEGER DEFAULT NULL,
    source_app      TEXT DEFAULT NULL,
    ocr_text        TEXT DEFAULT '',
    ocr_status      TEXT NOT NULL DEFAULT 'pending',
    is_pinned       INTEGER NOT NULL DEFAULT 0,
    group_name      TEXT NOT NULL DEFAULT '',
    last_copied_at  TEXT DEFAULT NULL,
    queued_delete_at TEXT DEFAULT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_captures_created_at ON captures(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_captures_size_bytes ON captures(size_bytes DESC);
CREATE INDEX IF NOT EXISTS idx_captures_type_pinned ON captures(media_type, is_pinned);

CREATE TABLE IF NOT EXISTS tags (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT NOT NULL UNIQUE COLLATE NOCASE,
    color           TEXT NOT NULL DEFAULT '#3b82f6'
);

CREATE TABLE IF NOT EXISTS capture_tags (
    capture_id      TEXT NOT NULL REFERENCES captures(id) ON DELETE CASCADE,
    tag_id          INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (capture_id, tag_id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS captures_fts USING fts5(
    capture_id UNINDEXED,
    source_app,
    ocr_text,
    tag_names,
    tokenize = 'porter unicode61'
);

CREATE TABLE IF NOT EXISTS clipboard_history (
    id                  TEXT PRIMARY KEY,
    blake3_hash         TEXT NOT NULL UNIQUE,
    file_path           TEXT NOT NULL,
    thumb_path          TEXT NOT NULL,
    base_file_path      TEXT DEFAULT NULL,
    annotations_json    TEXT NOT NULL DEFAULT '[]',
    width               INTEGER NOT NULL,
    height              INTEGER NOT NULL,
    size_bytes          INTEGER NOT NULL,
    is_pinned           INTEGER NOT NULL DEFAULT 0,
    copied_at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_clipboard_copied_at ON clipboard_history(is_pinned DESC, copied_at DESC);

CREATE TABLE IF NOT EXISTS settings (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL,
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
"#;

pub fn init_db(db_path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(SCHEMA_SQL)?;

    // Non-destructive column migrations for existing databases
    let _ = conn.execute(
        "ALTER TABLE captures ADD COLUMN group_name TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE captures ADD COLUMN last_copied_at TEXT DEFAULT NULL",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_captures_last_copied ON captures(last_copied_at DESC)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_captures_group_name ON captures(group_name)",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE captures ADD COLUMN ocr_status TEXT NOT NULL DEFAULT 'pending'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE captures ADD COLUMN queued_delete_at TEXT DEFAULT NULL",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_captures_queued_delete ON captures(queued_delete_at)",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE clipboard_history ADD COLUMN base_file_path TEXT DEFAULT NULL",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE clipboard_history ADD COLUMN annotations_json TEXT NOT NULL DEFAULT '[]'",
        [],
    );

    seed_default_settings(&conn)?;
    Ok(conn)
}

fn seed_default_settings(conn: &Connection) -> AppResult<()> {
    let defaults = [
        ("hotkey_region_capture", "\"Alt+S\""),
        ("hotkey_fullscreen_capture", "\"Alt+Shift+S\""),
        ("hotkey_open_gallery", "\"Alt+G\""),
        ("hotkey_visual_clipboard", "\"Alt+V\""),
        // Changed from Alt+R to Alt+C ("Save Clip") to avoid conflicting with NVIDIA's Alt+R overlay!
        ("hotkey_save_replay", "\"Alt+C\""),
        ("image_format", "\"webp\""),
        ("copy_to_clipboard_on_save", "true"),
        ("max_storage_gb", "10"),
        ("auto_cleanup_days", "0"),
        ("clipboard_max_items", "10"),
        ("clipboard_always_on_top", "true"),
        ("launch_on_startup", "false"),
    ];

    for (key, value) in defaults {
        conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
    }

    // One-time migration for existing databases that still had the old Phase 1.0 placeholder defaults (15 GB / 30 days)
    let already_migrated: bool = conn
        .query_row(
            "SELECT 1 FROM settings WHERE key = 'v1_5_3_defaults_migrated'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !already_migrated {
        let _ = conn.execute(
            "UPDATE settings SET value = '10' WHERE key = 'max_storage_gb' AND value = '15'",
            [],
        );
        let _ = conn.execute(
            "UPDATE settings SET value = '0' WHERE key = 'auto_cleanup_days' AND value = '30'",
            [],
        );
        let _ = conn.execute(
            "INSERT OR IGNORE INTO settings (key, value) VALUES ('v1_5_3_defaults_migrated', 'true')",
            [],
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_legacy_v1_db_migrates_cleanly_to_v1_5() {
        let temp_db = std::env::temp_dir().join(format!("rv_legacy_mig_{}.db", Uuid::new_v4()));

        // 1. Simulate a pre-existing v1.0 database without `ocr_status` or `queued_delete_at`
        {
            let conn = Connection::open(&temp_db).expect("open legacy db");
            conn.execute_batch(
                r#"
                CREATE TABLE captures (
                    id              TEXT PRIMARY KEY,
                    media_type      TEXT NOT NULL CHECK (media_type IN ('screenshot', 'video')),
                    file_path       TEXT NOT NULL,
                    thumb_path      TEXT NOT NULL,
                    width           INTEGER NOT NULL,
                    height          INTEGER NOT NULL,
                    size_bytes      INTEGER NOT NULL,
                    duration_ms     INTEGER DEFAULT NULL,
                    source_app      TEXT DEFAULT NULL,
                    ocr_text        TEXT DEFAULT '',
                    is_pinned       INTEGER NOT NULL DEFAULT 0,
                    created_at      TEXT NOT NULL
                );
                "#,
            )
            .expect("create legacy captures table");
        }

        // 2. Run `init_db` on the existing database — must migrate columns BEFORE creating indexes
        let migrated_conn = init_db(&temp_db).expect("init_db should migrate legacy v1.0 database");

        // 3. Verify `queued_delete_at` and `ocr_status` columns exist and are queryable
        let count: i64 = migrated_conn
            .query_row(
                "SELECT COUNT(*) FROM captures WHERE queued_delete_at IS NULL AND ocr_status = 'pending'",
                [],
                |r| r.get(0),
            )
            .expect("query migrated columns");
        assert_eq!(count, 0);

        drop(migrated_conn);
        let _ = std::fs::remove_file(&temp_db);
    }
}
