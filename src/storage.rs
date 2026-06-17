use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

use crate::clipboard_item::{ClipboardItem, ClipboardKind};

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open() -> Result<Self, Box<dyn Error>> {
        let data_dir = data_dir();
        fs::create_dir_all(data_dir.join("blobs").join("image"))?;
        fs::create_dir_all(data_dir.join("blobs").join("other"))?;

        let conn = Connection::open(data_dir.join("ccopy.db"))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let storage = Self { conn };
        storage.init()?;
        storage.compact_text_duplicates()?;
        Ok(storage)
    }

    fn init(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS clipboard_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                preview TEXT NOT NULL,
                text_content TEXT,
                plain_text TEXT,
                blob_path TEXT,
                format_name TEXT,
                mime_type TEXT,
                width INTEGER,
                height INTEGER,
                size_bytes INTEGER,
                hash TEXT NOT NULL,
                note TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_used_at INTEGER
            );

            CREATE TABLE IF NOT EXISTS clipboard_item_files (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                position INTEGER NOT NULL,
                FOREIGN KEY(item_id) REFERENCES clipboard_items(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                color TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS clipboard_item_tags (
                item_id INTEGER NOT NULL,
                tag_id INTEGER NOT NULL,
                PRIMARY KEY (item_id, tag_id),
                FOREIGN KEY(item_id) REFERENCES clipboard_items(id) ON DELETE CASCADE,
                FOREIGN KEY(tag_id) REFERENCES tags(id) ON DELETE CASCADE
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_clipboard_items_kind_hash
            ON clipboard_items(kind, hash);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_created_at
            ON clipboard_items(created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_updated_at
            ON clipboard_items(updated_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_last_used_at
            ON clipboard_items(last_used_at DESC);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_kind
            ON clipboard_items(kind);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_preview
            ON clipboard_items(preview);

            CREATE INDEX IF NOT EXISTS idx_clipboard_items_plain_text
            ON clipboard_items(plain_text);

            CREATE INDEX IF NOT EXISTS idx_clipboard_item_files_item_id
            ON clipboard_item_files(item_id);

            CREATE INDEX IF NOT EXISTS idx_clipboard_item_files_path
            ON clipboard_item_files(path);

            CREATE INDEX IF NOT EXISTS idx_tags_name
            ON tags(name);
            ",
        )
    }

    pub fn compact_text_duplicates(&self) -> rusqlite::Result<()> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, COALESCE(NULLIF(TRIM(plain_text), ''), NULLIF(TRIM(text_content), '')) AS visible_text
            FROM clipboard_items
            WHERE kind IN ('text', 'html', 'rtf')
            ORDER BY
                CASE kind WHEN 'html' THEN 0 WHEN 'rtf' THEN 1 ELSE 2 END,
                updated_at DESC,
                id DESC
            ",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?;

        let mut seen_text: HashMap<String, Vec<i64>> = HashMap::new();
        for row in rows {
            let (id, visible_text) = row?;
            if let Some(text) = visible_text {
                seen_text.entry(text).or_default().push(id);
            }
        }

        for (_, ids) in seen_text {
            for &id in &ids[1..] {
                self.conn
                    .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;
            }
        }

        Ok(())
    }

    pub fn upsert_item(&self, item: &ClipboardItem) -> rusqlite::Result<i64> {
        let existing_id = self
            .conn
            .query_row(
                "SELECT id FROM clipboard_items WHERE kind = ?1 AND hash = ?2",
                params![item.kind.as_str(), item.hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;

        if let Some(id) = existing_id {
            self.conn.execute(
                "
                UPDATE clipboard_items
                SET preview = ?1,
                    text_content = ?2,
                    plain_text = ?3,
                    blob_path = ?4,
                    format_name = ?5,
                    mime_type = ?6,
                    width = ?7,
                    height = ?8,
                    size_bytes = ?9,
                    note = ?10,
                    updated_at = ?11
                WHERE id = ?12
                ",
                params![
                    item.preview,
                    item.text_content,
                    item.plain_text,
                    item.blob_path,
                    item.format_name,
                    item.mime_type,
                    item.width,
                    item.height,
                    item.size_bytes,
                    item.note,
                    item.updated_at,
                    id
                ],
            )?;
            if !item.files.is_empty() {
                self.conn.execute(
                    "DELETE FROM clipboard_item_files WHERE item_id = ?1",
                    params![id],
                )?;
                for file in &item.files {
                    self.conn.execute(
                        "INSERT INTO clipboard_item_files (item_id, path, position) VALUES (?1, ?2, ?3)",
                        params![id, file.path, file.position],
                    )?;
                }
            }
            return Ok(id);
        }

        self.conn.execute(
            "
            INSERT INTO clipboard_items (
                kind, preview, text_content, plain_text, blob_path, format_name, mime_type,
                width, height, size_bytes, hash, note, created_at, updated_at, last_used_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ",
            params![
                item.kind.as_str(),
                item.preview,
                item.text_content,
                item.plain_text,
                item.blob_path,
                item.format_name,
                item.mime_type,
                item.width,
                item.height,
                item.size_bytes,
                item.hash,
                item.note,
                item.created_at,
                item.updated_at,
                item.last_used_at
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if !item.files.is_empty() {
            for file in &item.files {
                self.conn.execute(
                    "INSERT INTO clipboard_item_files (item_id, path, position) VALUES (?1, ?2, ?3)",
                    params![id, file.path, file.position],
                )?;
            }
        }
        Ok(id)
    }

    pub fn recent_items(&self, limit: usize) -> rusqlite::Result<Vec<ClipboardItem>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, kind, preview, text_content, plain_text, blob_path, format_name, mime_type,
                   width, height, size_bytes, hash, note, created_at, updated_at, last_used_at
            FROM clipboard_items
            ORDER BY updated_at DESC, id DESC
            LIMIT ?1
            ",
        )?;

        let mut items = Vec::new();
        let mut rows = stmt.query(params![limit as i64])?;
        while let Some(row) = rows.next()? {
            let kind_text: String = row.get(1)?;
            let kind = ClipboardKind::from_str(&kind_text).unwrap_or(ClipboardKind::Other);
            let mut item = ClipboardItem {
                id: row.get(0)?,
                kind,
                preview: row.get(2)?,
                text_content: row.get(3)?,
                plain_text: row.get(4)?,
                blob_path: row.get(5)?,
                format_name: row.get(6)?,
                mime_type: row.get(7)?,
                width: row.get(8)?,
                height: row.get(9)?,
                size_bytes: row.get(10)?,
                hash: row.get(11)?,
                note: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                last_used_at: row.get(15)?,
                files: Vec::new(),
                tags: Vec::new(),
            };

            if item.kind == ClipboardKind::Files {
                if let Some(item_id) = item.id {
                    let mut stmt_files = self.conn.prepare(
                        "SELECT path, position FROM clipboard_item_files WHERE item_id = ?1 ORDER BY position",
                    )?;
                    let mut rows_files = stmt_files.query(params![item_id])?;
                    while let Some(row_file) = rows_files.next()? {
                        item.files.push(crate::clipboard_item::ClipboardFile {
                            path: row_file.get(0)?,
                            position: row_file.get(1)?,
                        });
                    }
                }
            }

            items.push(item);
        }

        Ok(items)
    }

    pub fn mark_used(&self, id: i64, now: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE clipboard_items SET last_used_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn delete_item(&self, id: i64) -> rusqlite::Result<()> {
        let blob_path = self
            .conn
            .query_row(
                "SELECT blob_path FROM clipboard_items WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();

        self.conn
            .execute("DELETE FROM clipboard_items WHERE id = ?1", params![id])?;

        if let Some(blob_path) = blob_path {
            let _ = fs::remove_file(data_dir().join(blob_path));
        }

        Ok(())
    }

    pub fn update_note(&self, id: i64, note: Option<String>) -> rusqlite::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        self.conn.execute(
            "UPDATE clipboard_items SET note = ?1, updated_at = ?2 WHERE id = ?3",
            params![note, now, id],
        )?;
        Ok(())
    }
}

pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join("CCopy")
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library").join("Application Support").join("CCopy"))
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".config").join("CCopy"))
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}
